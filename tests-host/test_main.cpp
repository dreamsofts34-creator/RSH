// RSH host-side virtual tests — protocol + debounce + scheduler logic.
// Ye wohi header files istemal karte hain jo firmware compile karta hai.
#include <cstdio>
#include <cstring>
#include <cstdint>
#include <cstdlib>
#include "protocol.h"
#include "inputs.h"
#include "safemode.h"     // FW-3: boot counter + BOOT-hold + safe mode
#include "net_logic.h"    // FW-7: channel score/pick, pc_active, TxStats
#include "ctrl.h"        // FW-8: HMAC control dispatcher (dispatch tests §19)
#include "ota_validate.h" // FW-9: OTA magic/version/size + auth gate
#include "heap_policy.h"  // FW-10: largest-block + 5-min drop restart policy
#include "modules.h"      // FW-11: handbrake/shifter/horn, GET_CAPS, on/off
#include "encoders.h"     // FW-12: quadrature cumulative i16
#include "led_patterns.h" // FW-12: LED_TEST patterns + R-L9 pin rule
#include "led_driver.h"   // Phase G WS5: LED driver abstraction (sink boundary)
#include "ws2812_rmt.h"   // Phase H WS2: WS2812 RMT bitstream encoder
#include "heap_policy.h"    // FW-10 (D17)
#include "modules.h"        // FW-11 (handbrake/shifter/horn + GET_CAPS)
#include "encoders.h"       // FW-12 (quadrature, i16 cumulative)
#include "led_patterns.h"   // FW-12 (LED_TEST patterns + R-L9)
#include "safemode.h"       // FW-3 (Workstream 1: safe mode + boot counter)
#include "config_transaction.h"  // FW-4 (Workstream 1: config transaction)
#include "ctrl_handlers.h"   // FW-3/FW-4 dispatcher wiring (follow-up §21)
#include "password.h"        // SET_PASSWORD 0x0C validation (§5.2, Phase E WS5)
#include "pw_obfuscate.h"    // NVS password obfuscation (SEC-E-01, Phase F W1)
#include "stream_emit.h"     // v2.1 stream emission (§1.1, Phase E WS5)
#include "serial_transport.h" // wired USB-serial framing (§10, Phase G WS1)
#include "as5600.h"          // Phase G WS4: AS5600 magnetic encoder (D34)

static int failures = 0, checks = 0;
#define CHECK(cond) do { checks++; if (!(cond)) { failures++; \
  printf("FAIL %s:%d  %s\n", __FILE__, __LINE__, #cond); } } while (0)

// --- Independent table-driven CRC (protocol.h wale bitwise se alag tareeqa) ---
static uint16_t crc_table[256];
static void crc_table_init() {
  for (int i = 0; i < 256; i++) {
    uint16_t c = (uint16_t)i << 8;
    for (int b = 0; b < 8; b++) c = (c & 0x8000) ? (uint16_t)((c << 1) ^ 0x1021) : (uint16_t)(c << 1);
    crc_table[i] = c;
  }
}
static uint16_t crc_table_impl(const uint8_t *d, size_t n) {
  uint16_t crc = 0xFFFF;
  for (size_t i = 0; i < n; i++) crc = (uint16_t)((crc << 8) ^ crc_table[((crc >> 8) ^ d[i]) & 0xFF]);
  return crc;
}

// ---- Independent SHA-256 (test-only, FIPS 180-4) + HMAC-SHA256 -------------
// protocol.h mein crypto NAHI hai: firmware mbedTLS deta hai, host test ye
// independent implementation deta hai (rsh_hmac16_fn callback ke tor par).
#define ROTR32(x, n) (((x) >> (n)) | ((x) << (32 - (n))))
static const uint32_t sha_k[64] = {
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};
static void sha256_compress(uint32_t h[8], const uint8_t *p) {
  uint32_t w[64];
  for (int i = 0; i < 16; i++)
    w[i] = ((uint32_t)p[4 * i] << 24) | ((uint32_t)p[4 * i + 1] << 16) |
           ((uint32_t)p[4 * i + 2] << 8) | (uint32_t)p[4 * i + 3];
  for (int i = 16; i < 64; i++) {
    uint32_t s0 = ROTR32(w[i - 15], 7) ^ ROTR32(w[i - 15], 18) ^ (w[i - 15] >> 3);
    uint32_t s1 = ROTR32(w[i - 2], 17) ^ ROTR32(w[i - 2], 19) ^ (w[i - 2] >> 10);
    w[i] = w[i - 16] + s0 + w[i - 7] + s1;
  }
  uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
  for (int i = 0; i < 64; i++) {
    uint32_t S1 = ROTR32(e, 6) ^ ROTR32(e, 11) ^ ROTR32(e, 25);
    uint32_t ch = (e & f) ^ ((~e) & g);
    uint32_t t1 = hh + S1 + ch + sha_k[i] + w[i];
    uint32_t S0 = ROTR32(a, 2) ^ ROTR32(a, 13) ^ ROTR32(a, 22);
    uint32_t mj = (a & b) ^ (a & c) ^ (b & c);
    uint32_t t2 = S0 + mj;
    hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
  }
  h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}
static void sha256_once(const uint8_t *d, size_t n, uint8_t out[32]) {
  uint32_t h[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                   0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
  size_t padded = ((n + 9 + 63) / 64) * 64;
  uint8_t *m = (uint8_t *)malloc(padded);
  memcpy(m, d, n);
  m[n] = 0x80;
  memset(m + n + 1, 0, padded - n - 1);
  uint64_t bits = (uint64_t)n * 8;
  for (int i = 0; i < 8; i++) m[padded - 1 - i] = (uint8_t)(bits >> (8 * i));
  for (size_t off = 0; off < padded; off += 64) sha256_compress(h, m + off);
  free(m);
  for (int i = 0; i < 8; i++) {
    out[4 * i] = (uint8_t)(h[i] >> 24); out[4 * i + 1] = (uint8_t)(h[i] >> 16);
    out[4 * i + 2] = (uint8_t)(h[i] >> 8); out[4 * i + 3] = (uint8_t)h[i];
  }
}
// tag = HMAC-SHA256(key32, nonce16 || counter_LE || cmd || payload)[0:16] (§3.2)
static void test_hmac16(const uint8_t *key32, const uint8_t *nonce16, uint32_t counter,
                        uint8_t cmd, const uint8_t *payload, uint16_t payload_len,
                        uint8_t tag_out[16]) {
  size_t msg_len = 16 + 4 + 1 + payload_len;
  uint8_t *msg = (uint8_t *)malloc(msg_len);
  memcpy(msg, nonce16, 16);
  msg[16] = (uint8_t)counter; msg[17] = (uint8_t)(counter >> 8);
  msg[18] = (uint8_t)(counter >> 16); msg[19] = (uint8_t)(counter >> 24);
  msg[20] = cmd;
  if (payload_len) memcpy(msg + 21, payload, payload_len);
  uint8_t kipad[64], kopad[64];
  for (int i = 0; i < 64; i++) {
    uint8_t k = i < 32 ? key32[i] : 0;
    kipad[i] = (uint8_t)(k ^ 0x36); kopad[i] = (uint8_t)(k ^ 0x5c);
  }
  uint8_t *inner = (uint8_t *)malloc(64 + msg_len);
  memcpy(inner, kipad, 64); memcpy(inner + 64, msg, msg_len);
  uint8_t inner_hash[32];
  sha256_once(inner, 64 + msg_len, inner_hash);
  uint8_t *outer = (uint8_t *)malloc(64 + 32);
  memcpy(outer, kopad, 64); memcpy(outer + 64, inner_hash, 32);
  uint8_t full[32];
  sha256_once(outer, 64 + 32, full);
  memcpy(tag_out, full, 16);
  free(msg); free(inner); free(outer);
}

// pw_obfuscate.h callback (SEC-E-01, §24): test SHA-256 ko int-returning
// wrapper mein lapeto. Failing variant bhi hai (fail-closed check).
static int test_sha256_cb(const uint8_t *d, size_t n, uint8_t out[32]) {
  sha256_once(d, n, out);
  return 1;
}
static int test_sha256_fail_cb(const uint8_t *d, size_t n, uint8_t out[32]) {
  (void)d; (void)n; (void)out;
  return 0;
}

static uint32_t rng_state = 0x12345678;
static uint32_t rng() { rng_state = rng_state * 1664525u + 1013904223u; return rng_state >> 8; }

// FW-8 dispatcher test handler (ctrl.h, §19): GET_CAPS -> golden minimal
// body (17 B, pinCount 0 — protocol_v2_vectors.py ctrl_get_caps_reply
// payload se); GET_CFG -> 200 B body (cache-overflow path); REBOOT ->
// reboot flag; baqi -> UNSUPPORTED (execute nahi hota).
struct test_ctrl_ctx_t {
  int exec_caps;
  int exec_big;
  int reboot;
};
static uint8_t test_ctrl_handler(uint8_t cmd, const uint8_t *payload, uint16_t len,
                                 uint8_t *reply_body, uint16_t *reply_body_len, void *ctx) {
  (void)payload;
  (void)len;
  test_ctrl_ctx_t *c = (test_ctrl_ctx_t *)ctx;
  if (cmd == 0x01) {  // GET_CAPS
    c->exec_caps++;
    static const uint8_t golden_body[17] = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
      0x04, 0x00, 0x00, 0x02, 0x01, 0x08, 0x40, 0x04, 0x06, 0x00, 0x00};
    memcpy(reply_body, golden_body, 17);
    *reply_body_len = 17;
    return RSH_CTRL_OK;
  }
  if (cmd == 0x02) {  // GET_CFG — bara reply (cache overflow, > 128 B)
    c->exec_big++;
    for (int i = 0; i < 200; i++) reply_body[i] = (uint8_t)i;
    *reply_body_len = 200;
    return RSH_CTRL_OK;
  }
  if (cmd == 0x09) {  // REBOOT
    c->reboot++;
    *reply_body_len = 0;
    return RSH_CTRL_OK;
  }
  *reply_body_len = 0;
  return RSH_CTRL_UNSUPPORTED;
}

// FW-9 §20 helpers: libsodium sign (host decls) + recording hash sink.
// (verify ka declaration ota_validate.h mein hai — host branch.)
extern "C" {
int crypto_sign_seed_keypair(unsigned char *pk, unsigned char *sk, const unsigned char *seed);
int crypto_sign_detached(unsigned char *sig, unsigned long long *siglen_p, const unsigned char *m, unsigned long long mlen, const unsigned char *sk);
}
static uint8_t rec_buf[4096];
static size_t rec_len = 0;
static void rec_sink(void *ctx, const uint8_t *data, size_t len) {
  (void)ctx;
  memcpy(rec_buf + rec_len, data, len);
  rec_len += len;
}

// §21 fakes (FW-3/FW-4 dispatcher wiring): armed flag, fake clock (millis),
// NVS-persist spy (calls + aakhri pending value), KEY_RESET-request spy.
static uint8_t fw34_test_armed = 0;
static uint32_t fw34_test_now = 1000;
static int fw34_test_persist_calls = 0;
static uint8_t fw34_test_persist_last_pending = 0;
static int fw34_test_keyreset_calls = 0;
static uint8_t fw34_test_armed_cb(void *u) { (void)u; return fw34_test_armed; }
static void fw34_test_keyreset_cb(void *u) { (void)u; fw34_test_keyreset_calls++; }
static void fw34_test_persist_cb(void *u, uint8_t pending) {
  (void)u;
  fw34_test_persist_calls++;
  fw34_test_persist_last_pending = pending;
}
static uint32_t fw34_test_now_cb(void *u) { (void)u; return fw34_test_now; }
static int fw34_test_password_calls = 0;
static uint8_t fw34_test_password[64];
static uint16_t fw34_test_password_len = 0;
static void fw34_test_password_cb(void *u, const uint8_t *pw, uint16_t len) {
  (void)u;
  fw34_test_password_calls++;
  fw34_test_password_len = len;
  if (pw && len <= 64) memcpy(fw34_test_password, pw, len);
}
// Phase F WS5: LED_TEST callback probe (ctrl_handlers.h led_test_request).
static int fw34_test_led_calls = 0;
static RshLedPattern fw34_test_led_last;
static void fw34_test_led_cb(void *u, const RshLedPattern *pattern) {
  (void)u;
  fw34_test_led_calls++;
  if (pattern) fw34_test_led_last = *pattern;
}
// Phase G WS5: LED driver recording sink (led_driver.h) — har sink write
// ka frame record hota hai taake tick/change-detection verify ho sake.
static RshLedRgb led_sink_frames[64][RSH_LED_DRIVER_MAX];
static uint8_t led_sink_counts[64];
static int led_sink_writes = 0;
static void led_sink_rec(void *u, const RshLedRgb *px, uint8_t n) {
  (void)u;
  if (led_sink_writes < 64 && n <= RSH_LED_DRIVER_MAX) {
    led_sink_counts[led_sink_writes] = n;
    for (uint8_t i = 0; i < n; i++) led_sink_frames[led_sink_writes][i] = px[i];
  }
  led_sink_writes++;
}

// §24 mock I2C (AS5600, Phase G WS4): canned register bytes + fail mode.
static uint8_t as5600_mock_status = 0x20;  // MD=1
static uint16_t as5600_mock_raw = 0;
static uint8_t as5600_mock_agc = 64;
static int as5600_mock_fail = 0;
static int as5600_mock_read(uint8_t dev, uint8_t reg, uint8_t *buf,
                            uint8_t len, void *user) {
  (void)user;
  if (dev != RSH_AS5600_I2C_ADDR) return -1;
  if (as5600_mock_fail) return -1;
  if (reg == RSH_AS5600_REG_STATUS && len == 1) {
    buf[0] = as5600_mock_status;
    return 0;
  }
  if (reg == RSH_AS5600_REG_RAWANGLE_H && len == 2) {
    buf[0] = (uint8_t)((as5600_mock_raw >> 8) & 0x0F);
    buf[1] = (uint8_t)(as5600_mock_raw & 0xFF);
    return 0;
  }
  if (reg == RSH_AS5600_REG_AGC && len == 1) {
    buf[0] = as5600_mock_agc;
    return 0;
  }
  return -1;
}

int main() {
  crc_table_init();

  // 1) CRC standard check value (RevEng catalogue: CRC-16/CCITT-FALSE)
  const char *vec = "123456789";
  CHECK(rsh_crc16_ccitt((const uint8_t *)vec, 9) == 0x29B1);
  CHECK(crc_table_impl((const uint8_t *)vec, 9) == 0x29B1);
  // khaali data par init value hi rehti hai
  CHECK(rsh_crc16_ccitt((const uint8_t *)vec, 0) == 0xFFFF);

  // 2) Dono CRC implementations 500 random buffers par mutafiq hon
  for (int t = 0; t < 500; t++) {
    uint8_t buf[64];
    size_t n = rng() % 65;
    for (size_t i = 0; i < n; i++) buf[i] = (uint8_t)rng();
    CHECK(rsh_crc16_ccitt(buf, n) == crc_table_impl(buf, n));
  }

  // 3) Packet golden bytes — layout bilkul docs (02) ke mutabiq ho
  uint8_t pkt[RSH_PACKET_SIZE];
  size_t len = rsh_build_packet(pkt, 0x1234, 0x01020304, 2300, 1000, 2000, 0x05, 0x01);
  CHECK(len == 18);
  const uint8_t expect[16] = {0xA5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
                              0xFC, 0x08, 0xE8, 0x03, 0xD0, 0x07, 0x05, 0x01};
  CHECK(memcmp(pkt, expect, 16) == 0);
  uint16_t crc = crc_table_impl(pkt, 16);   // independent implementation se
  CHECK(pkt[16] == (crc & 0xFF) && pkt[17] == (crc >> 8));

  // 4) Packet ke har field ki boundary values (0 aur max)
  rsh_build_packet(pkt, 0xFFFF, 0xFFFFFFFFu, 4095, 0, 3300, 0xFF, 0xFF);
  CHECK(pkt[2] == 0xFF && pkt[3] == 0xFF);
  CHECK(pkt[8] == 0xFF && pkt[9] == 0x0F);          // 4095 = 0x0FFF
  CHECK(pkt[12] == 0xE4 && pkt[13] == 0x0C);        // 3300 = 0x0CE4
  CHECK(rsh_crc16_ccitt(pkt, 16) == (uint16_t)(pkt[16] | (pkt[17] << 8)));

  // 5) Debouncer — TIME-based (F12): raw 10 ms stable rahe to hi state badle.
  // Sample-count wala tareeqa khatam: governor level ke saath cycle 5->10 ms
  // hota hai, "2 samples" ka matlab badal jata tha.
  {
    Debouncer d;   // debounceMs = DEBOUNCE_MS = 10
    CHECK(d.update(false, 0) == false);
    CHECK(d.update(true, 5) == false);    // sirf 5 ms — abhi nahi
    CHECK(d.update(false, 6) == false);   // bounce — pending reset
    CHECK(d.update(true, 10) == false);   // pending shuru
    CHECK(d.update(true, 19) == false);   // 9 ms — abhi nahi
    CHECK(d.update(true, 20) == true);    // 10 ms poore — pressed
    CHECK(d.update(true, 100) == true);
    CHECK(d.update(false, 105) == true);  // release par bhi 10 ms chahiye
    CHECK(d.update(false, 115) == false);
    // alternating bounce (har 5 ms flip) kabhi state na badle
    Debouncer e;
    for (int i = 0; i < 20; i++) e.update(i % 2 == 0, (uint32_t)(i * 5));
    CHECK(e.state == false);
    // lamba press phir lamba release — dono waqt par badle
    Debouncer f;
    CHECK(f.update(true, 0) == false);
    CHECK(f.update(true, 10) == true);
    CHECK(f.update(false, 50) == true);
    CHECK(f.update(false, 60) == false);
  }

  // 6) Scheduler ka wrap-safe comparison (micros ~71.6 min par wrap hota hai)
  {
    uint32_t deadline = 0xFFFFFFF0u + 5000u;   // wrap ke paar chala gaya
    uint32_t before = 0xFFFFFFF0u;
    uint32_t atDue  = deadline;
    uint32_t after  = deadline + 100u;
    CHECK((int32_t)(before - deadline) < 0);    // abhi waqt nahi
    CHECK((int32_t)(atDue - deadline) >= 0);    // waqt aa gaya
    CHECK((int32_t)(after - deadline) >= 0);
  }

  // 7) Sequence wrap: 0xFFFF ke baad 0 (uint16 ka fitri rawaiya, PC isi par chalta hai)
  {
    uint16_t s = 0xFFFF;
    s++;
    CHECK(s == 0);
  }

  // 8) Trimmed mean: spike values trim ho jayen, aam average spike se bigarta hai
  {
    uint16_t steady[16];
    for (int i = 0; i < 16; i++) steady[i] = 2000;
    CHECK(rsh_trimmed_mean(steady, 16, 2) == 2000);

    uint16_t spiked[16];
    for (int i = 0; i < 16; i++) spiked[i] = 2000;
    spiked[3] = 4095;    // ek ADC spike
    spiked[9] = 0;       // ek dropout
    CHECK(rsh_trimmed_mean(spiked, 16, 2) == 2000);   // dono trim ho gaye

    uint16_t ramp[8] = {0, 100, 200, 300, 400, 500, 600, 700};
    // trim=1: 0 aur 700 hate, baqi 100..600 ka average = 350
    CHECK(rsh_trimmed_mean(ramp, 8, 1) == 350);
    // trim=0: plain average = 350 bhi yahan wahi (symmetric), farq spike par ata hai
    uint16_t one[1] = {1234};
    CHECK(rsh_trimmed_mean(one, 1, 0) == 1234);
  }

  // 8b) Trimmed mean guards (T14): n=0 / null / n>32 / trim*2>=n — UB nahi,
  // defined result. (Pehle: n=0 par s[0] uninitialized read, n>32 par s[32]
  // stack overflow — ASan/UBSan ke baghair bhi ye checks pakarte hain.)
  {
    CHECK(rsh_trimmed_mean(NULL, 0, 0) == 0);        // khaali input — koi read nahi
    uint16_t empty[1] = {0};
    CHECK(rsh_trimmed_mean(empty, 0, 0) == 0);
    CHECK(rsh_trimmed_mean(NULL, 5, 1) == 0);        // null pointer — crash nahi
    // n>32: sirf pehle 32 parhte hain (stack overflow nahi)
    uint16_t big[40];
    for (int i = 0; i < 40; i++) big[i] = 2000;
    CHECK(rsh_trimmed_mean(big, 40, 2) == 2000);
    // trim*2>=n: trim clamp hota hai, kam se kam ek element bachta hai
    uint16_t ramp2[8] = {0, 100, 200, 300, 400, 500, 600, 700};
    CHECK(rsh_trimmed_mean(ramp2, 8, 4) == 350);     // trim 4 -> 3: (300+400)/2
    uint16_t one2[1] = {1234};
    CHECK(rsh_trimmed_mean(one2, 1, 1) == 1234);     // trim 1 -> 0: average = 1234
  }

  // 8c) Pedal spike test (F6): pedal-sized window (n=8, trim=1) mein ek spike
  // ya dropout pedal ko nahi hilata — pehle plain mean hil jata tha.
  // (Naive reference: poora sort karke beech ka average.)
  {
    auto naive_trimmed = [](const uint16_t *v, int n, int trim) {
      uint16_t s[32];
      for (int i = 0; i < n; i++) s[i] = v[i];
      for (int i = 0; i < n; i++)
        for (int j = i + 1; j < n; j++)
          if (s[j] < s[i]) { uint16_t t = s[i]; s[i] = s[j]; s[j] = t; }
      uint32_t sum = 0;
      for (int i = trim; i < n - trim; i++) sum += s[i];
      return (uint16_t)(sum / (n - 2 * trim));
    };
    uint16_t pedal[8] = {1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000};
    pedal[2] = 4095;   // ek spike
    CHECK(rsh_trimmed_mean(pedal, 8, 1) == 1000);
    pedal[2] = 1000; pedal[6] = 0;   // ek dropout
    CHECK(rsh_trimmed_mean(pedal, 8, 1) == 1000);
    // Quickselect implementation 300 random vectors par naive se mutafiq ho
    // (duplicates + spikes samet) — F14 optimize ki correctness.
    for (int t = 0; t < 300; t++) {
      uint16_t v[16];
      int n = 1 + (int)(rng() % 16);
      int trim = (int)(rng() % 4);
      if (trim * 2 >= n) trim = (n - 1) / 2;
      for (int i = 0; i < n; i++) {
        uint32_t r = rng() % 100;
        if (r < 50) v[i] = 2000;                 // duplicates
        else if (r < 60) v[i] = 4095;            // spike
        else if (r < 70) v[i] = 0;               // dropout
        else v[i] = (uint16_t)(rng() % 4096);
      }
      CHECK(rsh_trimmed_mean(v, (uint8_t)n, (uint8_t)trim) ==
            naive_trimmed(v, n, trim));
    }
  }

  // 8d) Floating / disconnected axis detection (F2, corrected criteria D12)
  {
    // Still wheel: pot beech mein ruka, variance≈0, doosre axes bhi still —
    // KABHI invalid nahi (purana criteria yahan false positive deta tha).
    AxisHealth h;
    bool inv = false;
    for (int i = 0; i < 400; i++)
      inv = h.update(2000, 4095, false, (uint32_t)(i * 5));
    CHECK(inv == false);
    // Rail par stuck 10 s + doosre axes move kar rahe hon -> invalid.
    AxisHealth r;
    inv = false;
    for (int t = 0; t <= 10000; t += 5)
      inv = r.update(4095, 4095, true, (uint32_t)t);
    CHECK(inv == true);
    // Rail par stuck 10 s lekin doosre axes bhi still (board hi band/parked)
    // -> valid. 9.9 s par bhi abhi valid.
    AxisHealth r2;
    inv = false;
    for (int t = 0; t <= 9900; t += 5)
      inv = r2.update(0, 4095, true, (uint32_t)t);
    CHECK(inv == false);
    inv = false;
    for (int t = 0; t <= 10000; t += 5)
      inv = r2.update(0, 4095, false, (uint32_t)t);
    CHECK(inv == false);
    // Floating pin: be-tarteeb noise, bari |diff| -> jald invalid.
    AxisHealth f;
    inv = false;
    uint32_t seed = 0x9E3779B9u;
    for (int i = 0; i < 60 && !inv; i++) {
      seed = seed * 1664525u + 1013904223u;
      uint16_t v = (uint16_t)(1200 + (seed >> 16) % 1800);  // 1200..3000 jump
      inv = f.update(v, 4095, false, (uint32_t)(i * 5));
    }
    CHECK(inv == true);
    // Aam steering sweep (lock-to-lock 0.5 s, ~41 counts/sample) -> valid.
    AxisHealth s;
    inv = false;
    for (int i = 0; i < 100; i++) {
      uint16_t v = (uint16_t)(i * 41);
      inv = s.update(v, 4095, false, (uint32_t)(i * 5));
    }
    CHECK(inv == false);
  }

  // 8e) mV fallback / restore faisle (F13) + pedal button-mode cadence (F7)
  {
    CHECK(rsh_mv_should_fallback(8 * 2000, 0, 8) == true);   // raw bari, mV 0
    CHECK(rsh_mv_should_fallback(8 * 100, 0, 8) == false);   // raw bhi ~0 (pot 0 par)
    CHECK(rsh_mv_should_fallback(8 * 2000, 8 * 1500, 8) == false);
    CHECK(rsh_mv_should_restore(8 * 1500, 8) == true);       // mV wapas aa gaye
    CHECK(rsh_mv_should_restore(0, 8) == false);
    CHECK(rsh_mv_should_restore(8 * 10, 8) == false);        // sirf noise
    // Button mode: sirf har 20th cycle diagnostic read (10 Hz @ 200 Hz).
    CHECK(rsh_pedal_should_read(false, 0) == true);
    CHECK(rsh_pedal_should_read(false, 1) == false);
    CHECK(rsh_pedal_should_read(false, 19) == false);
    CHECK(rsh_pedal_should_read(false, 20) == true);
    CHECK(rsh_pedal_should_read(true, 7) == true);           // analog: har cycle
  }

  // 9) Adaptive sampling governor (D16/F5): dabao par step down (level 3 tak),
  // sakoon par step up. Level 3 = 4 samples/axis + 100 pps fallback.
  // Recovery: <=1 overrun wali window bhi goodWindows gine (F5 fix — pehle
  // 1 overrun bhi goodWindows reset kar deta tha, recovery kabhi nahi hoti thi).
  {
    SampleGovernor g;
    CHECK(g.level == 0 && g.steerSamples() == 16 && g.pedalSamples() == 8);
    CHECK(g.sendPeriodUs() == 5000 && g.fallback100pps() == false);
    g.update(3);                       // zyada overruns — neeche
    CHECK(g.level == 1 && g.steerSamples() == 12 && g.pedalSamples() == 6);
    g.update(10);
    CHECK(g.level == 2 && g.steerSamples() == 8 && g.pedalSamples() == 4);
    g.update(10);
    CHECK(g.level == 3 && g.steerSamples() == 4 && g.pedalSamples() == 4);
    CHECK(g.sendPeriodUs() == 10000 && g.fallback100pps() == true);
    g.update(10);                      // is se neeche nahi
    CHECK(g.level == 3);
    for (int i = 0; i < 4; i++) g.update(0);
    CHECK(g.level == 3);               // 4 saaf windows kaafi nahi
    g.update(0);                       // 5th saaf window — ek step upar
    CHECK(g.level == 2);
    g.update(1);                       // 1 overrun: level waisa hi, magar window good gine
    CHECK(g.level == 2);
    g.update(2);
    CHECK(g.level == 2);
    // Recovery sirf 1-overrun windows se bhi hoti hai (F5): 5 windows jin
    // mein 0 ya 1 overrun ho -> ek step upar.
    g.update(1); g.update(1); g.update(0);
    CHECK(g.level == 1);
    for (int i = 0; i < 5; i++) g.update(1);
    CHECK(g.level == 0);
    CHECK(g.sendPeriodUs() == 5000 && g.fallback100pps() == false);
  }

  // 10) Protocol v2.1 (WP3: T1 golden, T2 negative, T3 status field-level).
  // Golden bytes tests-host/protocol_v2_vectors.py (VECTORS/NEGATIVE) se hain.
  {
    // SHA-256 self-checks (FIPS 180-4 test vectors)
    uint8_t digest[32];
    sha256_once((const uint8_t *)"", 0, digest);
    const uint8_t empty_exp[32] = {0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14,
      0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4,
      0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55};
    CHECK(memcmp(digest, empty_exp, 32) == 0);
    sha256_once((const uint8_t *)"abc", 3, digest);
    const uint8_t abc_exp[32] = {0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
      0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3,
      0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad};
    CHECK(memcmp(digest, abc_exp, 32) == 0);

    // Table-driven CRC (protocol.h) independent table se mutafiq ho (200 random buffers)
    for (int t = 0; t < 200; t++) {
      uint8_t buf[64];
      size_t n = rng() % 65;
      for (size_t i = 0; i < n; i++) buf[i] = (uint8_t)rng();
      CHECK(rsh_crc16_ccitt(buf, n) == crc_table_impl(buf, n));
    }

    uint8_t test_key[32], test_nonce[16], wrong_key[32];
    for (int i = 0; i < 32; i++) { test_key[i] = (uint8_t)i; wrong_key[i] = 0xFF; }
    for (int i = 0; i < 16; i++) test_nonce[i] = (uint8_t)(0xA0 + i);

    // --- T1: stream v2.1 golden ---
    const uint8_t data_v2[36] = {0x52, 0x53, 0x48, 0x32, 0x21, 0x01, 0x34, 0x12,
      0x04, 0x03, 0x02, 0x01, 0x1f, 0x02, 0x05, 0x02, 0x00, 0x00, 0x00, 0x00,
      0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07, 0x00, 0x00, 0x00, 0x02, 0xe8, 0x03,
      0x06, 0xff, 0x69, 0xd1};
    rsh_v2_stream_t st;
    CHECK(rsh_v2_parse_stream(data_v2, sizeof(data_v2), &st) == RSH_V2_OK);
    CHECK(st.seq == 0x1234 && st.timestamp_us == 0x01020304u);
    CHECK(st.flags == 0x01 && st.axis_mask == 0x1F && st.n_enc == 2);
    CHECK(st.buttons == 0x00000205u);
    CHECK(st.axes[0] == 2300 && st.axes[1] == 1000 && st.axes[2] == 2000);
    CHECK(st.axes[3] == 0 && st.axes[4] == 512);
    CHECK(st.enc[0] == 1000 && st.enc[1] == -250);
    // build roundtrip golden se match kare
    {
      uint16_t axes[5] = {2300, 1000, 2000, 0, 512};
      int16_t enc[2] = {1000, -250};
      uint8_t built[64];
      size_t blen = rsh_v2_build_stream(built, 0x1234, 0x01020304, 0x01, 0x1F,
                                        axes, 2, enc, 0x205);
      CHECK(blen == sizeof(data_v2) && memcmp(built, data_v2, blen) == 0);
    }

    // --- T1: status golden base 34 B / extended 38 B ---
    const uint8_t status_base[34] = {0xa6, 0x21, 0x07, 0x00, 0x40, 0xe2, 0x01, 0x00,
      0xcc, 0x56, 0xac, 0x0f, 0x78, 0xda, 0x02, 0x00, 0x03, 0x00, 0x5c, 0x08,
      0x00, 0x01, 0xef, 0xbe, 0xad, 0xde, 0x05, 0x00, 0x02, 0x00, 0x01, 0x00,
      0x97, 0x00};
    const uint8_t status_ext[38] = {0xa6, 0x21, 0x08, 0x00, 0x41, 0xe2, 0x01, 0x00,
      0xce, 0x56, 0xaa, 0x0f, 0x78, 0xda, 0x02, 0x00, 0x03, 0x00, 0x5c, 0x08,
      0x00, 0x01, 0xef, 0xbe, 0xad, 0xde, 0x05, 0x00, 0x20, 0x00, 0x00, 0x00,
      0x02, 0x00, 0x01, 0x00, 0xe1, 0x1e};
    rsh_v2_status_t ss;
    CHECK(rsh_v2_parse_status(status_base, sizeof(status_base), &ss) == RSH_V2_OK);
    CHECK(ss.has_btns_ex == 0 && ss.seq == 7 && ss.uptime_ms == 123456u);
    CHECK(ss.rssi_dbm == -52 && ss.battery_pct == 86 && ss.supply_mv == 4012);
    CHECK(ss.heap == 187000u && ss.overruns == 3 && ss.max_cycle_us == 2140);
    CHECK(ss.governor == 0 && ss.reset_reason == 1 && ss.cfg_crc == 0xDEADBEEFu);
    CHECK(ss.cfg_ver == 5 && ss.seq_gap == 2 && ss.tx_fail == 1);  // 28-29 / 30-31
    CHECK(rsh_v2_parse_status(status_ext, sizeof(status_ext), &ss) == RSH_V2_OK);
    CHECK(ss.has_btns_ex == 1 && ss.seq == 8 && ss.btns_ex == 0x20);
    CHECK(ss.seq_gap == 2 && ss.tx_fail == 1);  // extended mein 32-33 / 34-35
    CHECK((ss.btns_ex & (1u << 5)) != 0);       // button 37 set
    // build roundtrip golden se match kare
    {
      rsh_v2_status_t s2;
      memset(&s2, 0, sizeof(s2));
      s2.seq = 7; s2.uptime_ms = 123456; s2.rssi_dbm = -52; s2.battery_pct = 86;
      s2.supply_mv = 4012; s2.heap = 187000; s2.overruns = 3; s2.max_cycle_us = 2140;
      s2.reset_reason = 1; s2.cfg_crc = 0xDEADBEEF; s2.cfg_ver = 5;
      s2.seq_gap = 2; s2.tx_fail = 1;
      uint8_t built[40];
      CHECK(rsh_v2_build_status(built, &s2) == 34 && memcmp(built, status_base, 34) == 0);
      s2.seq = 8; s2.uptime_ms = 123457; s2.rssi_dbm = -50; s2.supply_mv = 4010;
      s2.has_btns_ex = 1; s2.btns_ex = 0x20;
      CHECK(rsh_v2_build_status(built, &s2) == 38 && memcmp(built, status_ext, 38) == 0);
    }

    // --- T1: HELLO reply golden (92 B) ---
    const uint8_t hello_unpaired[92] = {0x52, 0x53, 0x48, 0x52, 0x02, 0x01, 0x09, 0x01,
      0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x04, 0x00, 0x00, 0x00, 0xa0, 0xa1,
      0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad,
      0xae, 0xaf, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
      0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
      0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x41, 0x62,
      0x33, 0x64, 0x45, 0x66, 0x36, 0x68, 0x4a, 0x6b, 0x39, 0x6d, 0x4e, 0x70,
      0x32, 0x72, 0x53, 0x74, 0x35, 0x76, 0x57, 0x78, 0x38, 0x59, 0x5a, 0x42};
    rsh_v2_hello_t hello;
    CHECK(rsh_v2_parse_hello(hello_unpaired, sizeof(hello_unpaired), &hello) == RSH_V2_OK);
    CHECK((hello.flags & RSH_V2_HELLO_UNPAIRED) != 0);
    CHECK((hello.flags & RSH_V2_HELLO_WINDOW_OPEN) != 0);
    CHECK(hello.session_echo == 1 && hello.fw_major == 4 && hello.fw_minor == 0);
    CHECK(memcmp(hello.board_mac, "\xaa\xbb\xcc\xdd\xee\xff", 6) == 0);
    CHECK(memcmp(hello.nonce, test_nonce, 16) == 0);
    CHECK(memcmp(hello.device_key, test_key, 32) == 0);
    CHECK(memcmp(hello.wifi_password, "Ab3dEf6hJk9mNp2rSt5vWx8Y", 24) == 0);
    // paired: key/password zero (header + nonce golden jaisa, baaqi zero, CRC 0x4524)
    {
      uint8_t paired[92];
      memcpy(paired, hello_unpaired, 92);
      paired[6] = 0x00; paired[7] = 0x00;
      memset(paired + 34, 0, 56);
      paired[90] = 0x24; paired[91] = 0x45;
      CHECK(rsh_v2_parse_hello(paired, 92, &hello) == RSH_V2_OK);
      CHECK(hello.flags == 0);
      uint8_t zeros[56] = {0};
      CHECK(memcmp(hello.device_key, zeros, 32) == 0);
      CHECK(memcmp(hello.wifi_password, zeros, 24) == 0);
    }

    // --- T1: control golden ---
    const uint8_t ctrl_caps[29] = {0x52, 0x53, 0x48, 0x43, 0x01, 0x01, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0xee, 0x8d, 0x10, 0x07, 0x93, 0x58,
      0xd9, 0x93, 0x6f, 0xe7, 0x43, 0x68, 0xf8, 0xb6, 0x61};
    const uint8_t ctrl_setcfg[53] = {0x52, 0x53, 0x48, 0x43, 0x03, 0x02, 0x00, 0x01,
      0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x40, 0x00, 0x10, 0x00, 0xeb,
      0x52, 0x52, 0x53, 0x48, 0x47, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x5e, 0x0c, 0x65, 0x66, 0xc9, 0xdd, 0x5e,
      0x79, 0x1b, 0xe0, 0xae, 0x3e, 0x77, 0x66, 0x2d, 0x4c};
    const uint8_t ctrl_reply[47] = {0x52, 0x53, 0x48, 0x43, 0x81, 0x01, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x12, 0x00, 0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
      0x04, 0x00, 0x00, 0x02, 0x01, 0x08, 0x40, 0x04, 0x06, 0x00, 0x00, 0x04,
      0xb2, 0x21, 0x24, 0x07, 0x63, 0x4f, 0xfd, 0x85, 0x52, 0xf4, 0x91, 0xba,
      0x7d, 0xcb, 0xc0};
    rsh_v2_ctrl_t cf;
    CHECK(rsh_v2_ctrl_verify(ctrl_caps, sizeof(ctrl_caps), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_OK);
    CHECK(cf.cmd == 0x01 && cf.req_id == 1 && cf.counter == 0 && cf.len == 0);
    CHECK(rsh_v2_ctrl_verify(ctrl_setcfg, sizeof(ctrl_setcfg), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_OK);
    CHECK(cf.cmd == 0x03 && cf.req_id == 2 && cf.counter == 1 && cf.len == 24);
    CHECK(rsh_get_u16(cf.payload + 0) == 0 && rsh_get_u16(cf.payload + 2) == 64);
    CHECK(rsh_get_u16(cf.payload + 4) == 16);
    CHECK(memcmp(cf.payload + 8, "RSHG", 4) == 0);
    CHECK(rsh_v2_ctrl_verify(ctrl_reply, sizeof(ctrl_reply), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_OK);
    CHECK(cf.cmd == 0x81 && cf.payload[0] == RSH_CTRL_OK && cf.len == 18);
    CHECK(cf.payload[13] == 64);  // nBtnMax (wire)
    // build roundtrip golden se match kare
    {
      uint8_t built[64];
      size_t blen = rsh_v2_ctrl_build(built, sizeof(built), 0x01, 1, 0, NULL, 0,
                                     test_key, test_nonce, test_hmac16);
      CHECK(blen == sizeof(ctrl_caps) && memcmp(built, ctrl_caps, blen) == 0);
    }

    // --- T2: stream negatives ---
    {
      uint8_t bad[64];
      memcpy(bad, data_v2, sizeof(data_v2));
      bad[0] = 'X';
      CHECK(rsh_v2_parse_stream(bad, sizeof(data_v2), &st) == RSH_V2_BAD_MAGIC);
      memcpy(bad, data_v2, sizeof(data_v2));
      bad[4] = 0x20;
      CHECK(rsh_v2_parse_stream(bad, sizeof(data_v2), &st) == RSH_V2_BAD_VERSION);
      memcpy(bad, data_v2, sizeof(data_v2));
      bad[20] ^= 0xFF;  // axis badla, CRC purana
      CHECK(rsh_v2_parse_stream(bad, sizeof(data_v2), &st) == RSH_V2_BAD_CRC);
      CHECK(rsh_v2_parse_stream(data_v2, sizeof(data_v2) - 2, &st) == RSH_V2_BAD_FRAME);
      CHECK(rsh_v2_parse_stream(data_v2, 10, &st) == RSH_V2_TOO_SHORT);
      memcpy(bad, data_v2, sizeof(data_v2));
      bad[13] = 5;  // nEnc > 4
      CHECK(rsh_v2_parse_stream(bad, sizeof(data_v2), &st) == RSH_V2_BAD_FRAME);
      // reserved nonzero (CRC dobara lagao taake sirf reserved fail ho)
      uint8_t rsv[32];
      size_t rlen = rsh_v2_build_stream(rsv, 1, 100, 0, 0x00, NULL, 0, NULL, 0);
      rsv[18] = 0x01;
      rsh_put_u16(rsv + rlen - 2, rsh_crc16_ccitt(rsv, rlen - 2));
      CHECK(rsh_v2_parse_stream(rsv, rlen, &st) == RSH_V2_RSV_NONZERO);
      // flags reserved bits
      uint8_t flg[32];
      size_t flen = rsh_v2_build_stream(flg, 1, 100, 0, 0x00, NULL, 0, NULL, 0);
      flg[5] = 0x80;
      rsh_put_u16(flg + flen - 2, rsh_crc16_ccitt(flg, flen - 2));
      CHECK(rsh_v2_parse_stream(flg, flen, &st) == RSH_V2_RSV_NONZERO);
      // legacy v1 → v1 parser ka rasta
      const uint8_t v1gold[18] = {0xa5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
        0xfc, 0x08, 0xe8, 0x03, 0xd0, 0x07, 0x05, 0x01, 0x75, 0xbd};
      CHECK(rsh_v2_parse_stream(v1gold, sizeof(v1gold), &st) == RSH_V2_LEGACY_V1);
    }

    // --- T2: control negatives ---
    {
      uint8_t bad[64];
      memcpy(bad, ctrl_caps, sizeof(ctrl_caps));
      bad[sizeof(ctrl_caps) - 1] ^= 0xFF;  // bad tag
      CHECK(rsh_v2_ctrl_verify(bad, sizeof(ctrl_caps), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_TAG);
      CHECK(rsh_v2_ctrl_verify(ctrl_caps, sizeof(ctrl_caps), wrong_key, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_TAG);
      CHECK(rsh_v2_ctrl_verify(ctrl_caps, sizeof(ctrl_caps) - 8, test_key, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_LENGTH);
      memcpy(bad, ctrl_caps, sizeof(ctrl_caps));
      bad[0] = 'X'; bad[1] = 'X'; bad[2] = 'X'; bad[3] = 'X';
      CHECK(rsh_v2_ctrl_verify(bad, sizeof(ctrl_caps), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_MAGIC);
      // len field 5 kar do (asal 0) → length mismatch
      memcpy(bad, ctrl_caps, sizeof(ctrl_caps));
      bad[11] = 5;
      CHECK(rsh_v2_ctrl_verify(bad, sizeof(ctrl_caps), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_LENGTH);
      // oversize payload build mana hai
      uint8_t big[1100] = {0};
      uint8_t out[1200];
      CHECK(rsh_v2_ctrl_build(out, sizeof(out), 0x01, 1, 0, big, 1025,
                              test_key, test_nonce, test_hmac16) == 0);
      // structure parse tag ke baghair bhi reqId/counter deta hai (§3.2 BAD_TAG reply rule)
      memcpy(bad, ctrl_caps, sizeof(ctrl_caps));
      bad[sizeof(ctrl_caps) - 1] ^= 0xFF;
      CHECK(rsh_v2_ctrl_parse_structure(bad, sizeof(ctrl_caps), &cf) == RSH_V2_OK);
      CHECK(cf.req_id == 1 && cf.counter == 0);
    }

    // --- T2: replay / duplicate (tag valid, state rule reject karta hai) ---
    {
      const uint8_t replay[29] = {0x52, 0x53, 0x48, 0x43, 0x01, 0x63, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0xee, 0x8d, 0x10, 0x07, 0x93, 0x58,
        0xd9, 0x93, 0x6f, 0xe7, 0x43, 0x68, 0xf8, 0xb6, 0x61};
      // replay frame cryptographically verify hota hai (reqId HMAC mein nahi)
      CHECK(rsh_v2_ctrl_verify(replay, sizeof(replay), test_key, test_nonce, test_hmac16, &cf) == RSH_V2_OK);
      rsh_ctrl_session_t sess;
      rsh_ctrl_session_init(&sess);
      CHECK(rsh_ctrl_session_classify(&sess, 1, 0) == RSH_DEC_ACCEPT);
      CHECK(rsh_ctrl_session_classify(&sess, 1, 0) == RSH_DEC_DUPLICATE);
      CHECK(rsh_ctrl_session_classify(&sess, 99, 0) == RSH_DEC_REPLAY);
      CHECK(rsh_ctrl_session_classify(&sess, 2, 1) == RSH_DEC_ACCEPT);
      CHECK(rsh_ctrl_session_classify(&sess, 3, 5) == RSH_DEC_ACCEPT);  // gap ok
      CHECK(rsh_ctrl_session_classify(&sess, 4, 3) == RSH_DEC_REPLAY);
      CHECK(rsh_ctrl_session_classify(&sess, 3, 5) == RSH_DEC_DUPLICATE);
    }

    // --- T2: button count rule N1 ---
    CHECK(rsh_v2_check_button_count(0) == RSH_V2_OK);
    CHECK(rsh_v2_check_button_count(64) == RSH_V2_OK);
    CHECK(rsh_v2_check_button_count(65) == RSH_V2_UNSUPPORTED);
    CHECK(rsh_v2_check_button_count(128) == RSH_V2_UNSUPPORTED);
    CHECK(rsh_v2_check_button_count(129) == RSH_V2_BAD_FRAME);

    // --- T3: status field-level ---
    {
      CHECK(rsh_v2_parse_status(status_base, 33, &ss) == RSH_V2_TOO_SHORT);
      uint8_t len35[35];
      memcpy(len35, status_base, 34);
      CHECK(rsh_v2_parse_status(len35, 35, &ss) == RSH_V2_BAD_LENGTH);
      CHECK(rsh_v2_parse_status(status_base, 39, &ss) == RSH_V2_BAD_LENGTH);
      uint8_t bad[40];
      memcpy(bad, status_base, 34);
      bad[0] = 0xA5;
      CHECK(rsh_v2_parse_status(bad, 34, &ss) == RSH_V2_BAD_MAGIC);
      memcpy(bad, status_base, 34);
      bad[1] = 0x20;
      CHECK(rsh_v2_parse_status(bad, 34, &ss) == RSH_V2_BAD_VERSION);
      memcpy(bad, status_base, 34);
      bad[12] ^= 0xFF;
      CHECK(rsh_v2_parse_status(bad, 34, &ss) == RSH_V2_BAD_CRC);
      // wrap values + edge fields
      rsh_v2_status_t s2;
      memset(&s2, 0, sizeof(s2));
      s2.seq = 0xFFFF; s2.uptime_ms = 0xFFFFFFFF; s2.rssi_dbm = -128;
      s2.battery_pct = 255; s2.supply_mv = 0; s2.overruns = 0xFFFF;
      s2.max_cycle_us = 0xFFFF; s2.governor = 2; s2.reset_reason = 4;
      s2.cfg_ver = 0xFFFF; s2.seq_gap = 0xFFFF; s2.tx_fail = 0xFFFF;
      uint8_t built[40];
      CHECK(rsh_v2_build_status(built, &s2) == 34);
      CHECK(rsh_v2_parse_status(built, 34, &ss) == RSH_V2_OK);
      CHECK(ss.seq_gap == 0xFFFF && ss.tx_fail == 0xFFFF);
      CHECK(ss.battery_pct == 255 && ss.rssi_dbm == -128 && ss.supply_mv == 0);
      // base vs extended: aik hi values, sirf offsets badalte hain
      memset(&s2, 0, sizeof(s2));
      s2.seq = 9; s2.uptime_ms = 1000; s2.rssi_dbm = -60; s2.battery_pct = 50;
      s2.supply_mv = 4000; s2.heap = 100; s2.overruns = 1; s2.max_cycle_us = 100;
      s2.reset_reason = 1; s2.cfg_crc = 42; s2.cfg_ver = 3; s2.seq_gap = 7; s2.tx_fail = 9;
      uint8_t bb[40], ee[40];
      CHECK(rsh_v2_build_status(bb, &s2) == 34);
      s2.has_btns_ex = 1; s2.btns_ex = 0x01;
      CHECK(rsh_v2_build_status(ee, &s2) == 38);
      rsh_v2_status_t pb, pe;
      CHECK(rsh_v2_parse_status(bb, 34, &pb) == RSH_V2_OK);
      CHECK(rsh_v2_parse_status(ee, 38, &pe) == RSH_V2_OK);
      CHECK(pb.seq_gap == pe.seq_gap && pb.tx_fail == pe.tx_fail);
      CHECK(pe.seq_gap == 7 && pe.tx_fail == 9 && pe.btns_ex == 0x01);
    }

    // --- T3: PIN_PROBE frame (§3.8) ---
    {
      // mode 1, 3 samples mV: RSHP | 21 | seq 3 | pin 21 | mode 1 | count 3 | samples | CRC
      uint8_t probe[11 + 6 + 2];
      probe[0] = 'R'; probe[1] = 'S'; probe[2] = 'H'; probe[3] = 'P';
      probe[4] = 0x21;
      rsh_put_u16(probe + 5, 3);
      probe[7] = 21; probe[8] = 1;
      rsh_put_u16(probe + 9, 3);
      rsh_put_u16(probe + 11, 1200);
      rsh_put_u16(probe + 13, 3300);
      rsh_put_u16(probe + 15, 0);
      rsh_put_u16(probe + 17, rsh_crc16_ccitt(probe, 17));
      rsh_v2_probe_t pf;
      CHECK(rsh_v2_parse_probe(probe, sizeof(probe), &pf) == RSH_V2_OK);
      CHECK(pf.seq == 3 && pf.pin == 21 && pf.mode == 1 && pf.sample_count == 3);
      CHECK(pf.samples_mv[0] == 1200 && pf.samples_mv[1] == 3300 && pf.samples_mv[2] == 0);
      probe[11] ^= 0xFF;
      CHECK(rsh_v2_parse_probe(probe, sizeof(probe), &pf) == RSH_V2_BAD_CRC);
      // sampleCount 0 → BadFrame
      uint8_t zero[13];
      memcpy(zero, probe, 11);
      rsh_put_u16(zero + 9, 0);
      rsh_put_u16(zero + 11, rsh_crc16_ccitt(zero, 11));
      CHECK(rsh_v2_parse_probe(zero, sizeof(zero), &pf) == RSH_V2_BAD_FRAME);
    }
  }

  // 11) FW-10 heap policy (D17) + FW-11 modules + FW-12 LED/encoders.
  {
    // --- FW-10: normal run mein restart nahi ---
    HeapPolicy hp;
    heap_policy_reset(&hp);
    for (int i = 0; i < 100; i++)
      CHECK(heap_policy_update(&hp, 180000, 100000, (uint32_t)(i * 10000)) == 0);
    // --- FW-10: fragmentation inject -> 10 lagatar low cycles par restart ---
    heap_policy_reset(&hp);
    for (int i = 0; i < 9; i++)
      CHECK(heap_policy_update(&hp, 180000, 7000, (uint32_t)(i * 10000)) == 0);
    CHECK(heap_policy_update(&hp, 180000, 7000, 90000) == 1);  // 10th cycle
    CHECK(heap_policy_update(&hp, 180000, 100000, 100000) == 1);  // latched
    // --- FW-10: ek saaf cycle streak tor deta hai ---
    heap_policy_reset(&hp);
    for (int i = 0; i < 9; i++)
      CHECK(heap_policy_update(&hp, 180000, 7000, (uint32_t)(i * 10000)) == 0);
    CHECK(heap_policy_update(&hp, 180000, 20000, 90000) == 0);  // streak reset
    for (int i = 0; i < 9; i++)
      CHECK(heap_policy_update(&hp, 180000, 7000, (uint32_t)(100000 + i * 10000)) == 0);
    CHECK(heap_policy_update(&hp, 180000, 7000, 190000) == 1);  // dobara 10
    // --- FW-10: free heap 5 min mein 20% se zyada gire -> restart ---
    heap_policy_reset(&hp);
    CHECK(heap_policy_update(&hp, 200000, 100000, 0) == 0);       // baseline
    CHECK(heap_policy_update(&hp, 170000, 100000, 60000) == 0);   // -15% ok
    CHECK(heap_policy_update(&hp, 150000, 100000, 120000) == 1);  // -25% restart
    // Boundary: exactly 20% drop restart NAHI (drop > 20% chahiye).
    heap_policy_reset(&hp);
    CHECK(heap_policy_update(&hp, 200000, 100000, 0) == 0);
    CHECK(heap_policy_update(&hp, 160000, 100000, 60000) == 0);   // -20% ok
    CHECK(heap_policy_update(&hp, 159999, 100000, 120000) == 1);  // -20.0005%
    // --- FW-10: window khatam ho to re-baseline (slow legit girawat ok) ---
    heap_policy_reset(&hp);
    CHECK(heap_policy_update(&hp, 200000, 100000, 0) == 0);
    CHECK(heap_policy_update(&hp, 170000, 100000, 300000) == 0);  // window end
    CHECK(heap_policy_update(&hp, 140000, 100000, 360000) == 0);  // -17.6% of 170k
    CHECK(heap_policy_update(&hp, 130000, 100000, 420000) == 1);  // -23.5% restart
    // Baseline 0 guard: pehla sample 0 ho to bhi koi crash/restart nahi.
    heap_policy_reset(&hp);
    CHECK(heap_policy_update(&hp, 0, 0, 0) == 0);
    CHECK(heap_policy_update(&hp, 180000, 100000, 10000) == 0);
    CHECK(heap_policy_update(&hp, 0, 0, 20000) == 0);  // largest 0 -> streak 1

    // --- Round 2 W3 FIX-1: GET_CAPS/HELLO version FIRMWARE_VERSION se match ---
    // RSH_CAPS_FW_* hi net.cpp HELLO reply ka bhi source hai (single source
    // of truth) — 4.0.0 hard-code dobara na aaye, is liye constants + string
    // dono yahan bandhe hain.
    CHECK(RSH_CAPS_FW_MAJOR == 4 && RSH_CAPS_FW_MINOR == 0 && RSH_CAPS_FW_PATCH == 1);
    CHECK(strcmp(FIRMWARE_VERSION, "4.0.1") == 0);

    // --- FW-11: default pin table mein naye modules ki rows ---
    RshCapsEntry table[20];
    uint8_t ntab = rsh_caps_default_table(table, 20);
    CHECK(ntab == 15);
    int handbrakeRow = -1, hornRow = -1, ledRow = -1, shifterRows = 0, encRows = 0;
    uint8_t seenPins[40] = {0};
    for (int i = 0; i < ntab; i++) {
      CHECK(table[i].pin < 40 && !seenPins[table[i].pin]);  // koi pin do dafa nahi
      seenPins[table[i].pin] = 1;
      CHECK(table[i].reserved[0] == 0 && table[i].reserved[3] == 0);
      if (table[i].module == RSH_MOD_HANDBRAKE) {
        handbrakeRow = i;
        CHECK(table[i].pin == PIN_HANDBRAKE_ADC && table[i].kind == RSH_KIND_ADC_NATIVE);
      }
      if (table[i].module == RSH_MOD_HORN) {
        hornRow = i;
        CHECK(table[i].pin == PIN_HORN_BTN && table[i].kind == RSH_KIND_DIGITAL_IN);
      }
      if (table[i].module == RSH_MOD_LED) {
        ledRow = i;
        CHECK(table[i].pin == PIN_LED_STRIP && table[i].kind == RSH_KIND_LED_STRIP);
        CHECK(table[i].pin == 21);  // R-L9: LED strip GPIO21 par
      }
      if (table[i].module == RSH_MOD_SHIFTER) shifterRows++;
      if (table[i].module == RSH_MOD_ENCODERS) encRows++;
    }
    CHECK(handbrakeRow >= 0 && hornRow >= 0 && ledRow >= 0);
    CHECK(shifterRows == 2 && encRows == 4);
    CHECK(rsh_caps_default_table(table, 5) == 0);   // chhota buffer -> 0
    CHECK(rsh_caps_default_table(NULL, 20) == 0);

    // --- FW-11: GET_CAPS body build (§3.4) — naye modules ki rows bytes mein ---
    {
      uint8_t mac[6] = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF};
      uint8_t body[256];
      size_t blen = rsh_caps_build_body(body, sizeof(body), mac, 6, 0, table, ntab);
      CHECK(blen == 18 + 8u * ntab);
      CHECK(body[0] == RSH_CTRL_OK);
      CHECK(memcmp(body + 1, mac, 6) == 0);
      CHECK(body[7] == 4 && body[8] == 0 && body[9] == 1);   // fw 4.0.1 (Round 2 W3 FIX-1)
      CHECK(body[10] == 2 && body[11] == 1);                 // proto 2.1
      CHECK(body[12] == 8 && body[13] == 64 && body[14] == 4);
      CHECK(body[15] == 6 && body[16] == 0 && body[17] == ntab);
      // Handbrake row bytes (pin/kind/module/caps/reserved).
      const uint8_t *e = body + 18 + (size_t)handbrakeRow * 8;
      CHECK(e[0] == 32 && e[1] == RSH_KIND_ADC_NATIVE && e[2] == RSH_MOD_HANDBRAKE);
      CHECK(e[3] == (RSH_CAP_INPUT | RSH_CAP_ADC | RSH_CAP_PULLUP));
      CHECK(e[4] == 0 && e[5] == 0 && e[6] == 0 && e[7] == 0);
      // LED row bytes.
      e = body + 18 + (size_t)ledRow * 8;
      CHECK(e[0] == 21 && e[1] == RSH_KIND_LED_STRIP && e[2] == RSH_MOD_LED);
      CHECK(e[3] == RSH_CAP_OUTPUT);
      // Errors: chhota buffer, pinCount > 40.
      CHECK(rsh_caps_build_body(body, 20, mac, 6, 0, table, ntab) == 0);
      CHECK(rsh_caps_build_body(body, sizeof(body), mac, 6, 0, table, 41) == 0);
      // capsFlags bits 4-7 mask hote hain (reserved 0).
      CHECK(rsh_caps_build_body(body, sizeof(body), mac, 6, 0xF3, table, 1) == 26);
      CHECK(body[16] == 0x03);
    }

    // --- FW-11: module on/off — band module ka data packet mein nahi ---
    {
      // Phase F WS5: 7 fields (clutch off, sequential mode — default).
      // Handbrake slot 4 hai (pehle 3 tha — clutch ne slot 3 liya), is
      // liye sab-on mask 0x17 hai (1+2+4+16), 0x0F nahi.
      RshModuleEnable all = {1, 1, 1, 1, 1, 0, 0};
      CHECK(rsh_stream_axis_mask(&all) == 0x17);  // steer+accel+brake+handbrake(slot 4)
      RshModuleEnable noHb = {0, 1, 1, 1, 1, 0, 0};
      CHECK(rsh_stream_axis_mask(&noHb) == 0x07);  // handbrake bit clear
      CHECK(rsh_stream_axis_mask(NULL) == 0x07);   // NULL = sab band jaisa
      // Buttons: horn/shifter band hon to bits clear, baaqi nahi.
      // Aakhri arg shifterGear (H-pattern; sequential mein 0).
      CHECK(rsh_stream_button_bits(&all, 1, 1, 1, 1, 1, 0) == 0x1F);
      RshModuleEnable noHorn = {1, 1, 0, 1, 1, 0, 0};
      CHECK(rsh_stream_button_bits(&noHorn, 1, 1, 1, 1, 1, 0) == 0x0F);
      RshModuleEnable noShift = {1, 0, 1, 1, 1, 0, 0};
      CHECK(rsh_stream_button_bits(&noShift, 1, 1, 1, 1, 1, 0) == 0x1C);
      CHECK(rsh_stream_button_bits(&all, 0, 0, 1, 0, 0, 0) == 0x04);  // sirf accel
      // Clutch on: slot 3 bit set hota hai (mask 0x1F).
      RshModuleEnable withClutch = {1, 1, 1, 1, 1, 1, 0};
      CHECK(rsh_stream_axis_mask(&withClutch) == 0x1F);
      // H-pattern: gear 3 -> bit 7 (4+3), up/down bits clear.
      RshModuleEnable hpat = {1, 1, 1, 1, 1, 0, 1};
      CHECK(rsh_stream_button_bits(&hpat, 1, 1, 1, 1, 1, 3) == 0x9C);
      CHECK(rsh_stream_button_bits(&hpat, 1, 1, 1, 1, 1, 0) == 0x1C);  // neutral: koi gear bit nahi
      CHECK(rsh_stream_button_bits(&hpat, 1, 1, 1, 1, 1, 7) == 0x1C);  // gear 7 invalid
      // Enable state roundtrip + restore (baad ke tests default par chalein).
      rsh_modules_set_enabled(&noHb);
      CHECK(rsh_modules_handbrake_enabled() == 0);
      CHECK(rsh_modules_horn_enabled() == 1);
      rsh_modules_set_enabled(&hpat);
      CHECK(rsh_modules_shifter_mode() == 1);
      CHECK(rsh_modules_clutch_enabled() == 0);
      rsh_modules_set_enabled(&withClutch);
      CHECK(rsh_modules_clutch_enabled() == 1);
      CHECK(rsh_modules_shifter_mode() == 0);
      rsh_modules_set_enabled(&all);
      CHECK(rsh_modules_handbrake_enabled() == 1);
      CHECK(rsh_modules_clutch_enabled() == 0);
      CHECK(rsh_modules_shifter_mode() == 0);
    }

    // --- FW-11: H-pattern decode ---
    CHECK(rsh_shifter_hpattern_gear(0x00) == 0);  // neutral
    CHECK(rsh_shifter_hpattern_gear(0x01) == 1);
    CHECK(rsh_shifter_hpattern_gear(0x02) == 2);
    CHECK(rsh_shifter_hpattern_gear(0x20) == 6);
    CHECK(rsh_shifter_hpattern_gear(0x03) == 0);  // 2 gears — invalid
    CHECK(rsh_shifter_hpattern_gear(0x3F) == 0);
    CHECK(rsh_shifter_hpattern_gear(0xFF) == 0);  // upper bits mask hote hain
    // H-pattern + encoders conflict (shared pins 16/17/19).
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_HPATTERN, 1) == 1);
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_SEQUENTIAL, 1) == 0);
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_HPATTERN, 0) == 0);

    // --- FW-12: encoders — 100 turns aage + 100 peeche = exact 0 drift ---
    {
      RshEncoder enc;
      rsh_encoder_init(&enc);
      CHECK(enc.count == 0);
      // Pehla sample count nahi badalta.
      rsh_encoder_step(&enc, 0, 0);
      CHECK(enc.count == 0);
      // Forward quadrature cycle (a,b): 00 -> 10 -> 11 -> 01 -> 00 = +4.
      const uint8_t fwd[5][2] = {{0, 0}, {1, 0}, {1, 1}, {0, 1}, {0, 0}};
      for (int turn = 0; turn < 100; turn++) {
        for (int detent = 0; detent < 24; detent++) {
          for (int s = 1; s < 5; s++) rsh_encoder_step(&enc, fwd[s][0], fwd[s][1]);
        }
      }
      CHECK(enc.count == 9600);   // 100 turns x 24 detents x 4 = 9600, 0 drift
      // Reverse: 00 -> 01 -> 11 -> 10 -> 00 = -4.
      const uint8_t rev[5][2] = {{0, 0}, {0, 1}, {1, 1}, {1, 0}, {0, 0}};
      for (int turn = 0; turn < 100; turn++) {
        for (int detent = 0; detent < 24; detent++) {
          for (int s = 1; s < 5; s++) rsh_encoder_step(&enc, rev[s][0], rev[s][1]);
        }
      }
      CHECK(enc.count == 0);   // wapas exact 0 — 200 turns mein 0 drift
      // Bounce/glitch: dono bits ek saath badlen -> count nahi badalta.
      // (Naya encoder — reverse test ka prev state yahan uljha deta.)
      rsh_encoder_init(&enc);
      rsh_encoder_step(&enc, 0, 0);   // pehla sample, prev=00
      rsh_encoder_step(&enc, 1, 1);   // 00 -> 11 invalid (dono bits)
      CHECK(enc.count == 0);
      rsh_encoder_step(&enc, 0, 0);   // 11 -> 00 invalid (dono bits)
      CHECK(enc.count == 0);
      // 01 -> 10 invalid: pehle valid step se prev=01 par jao.
      rsh_encoder_step(&enc, 0, 1);   // 00 -> 01 valid, count=-1
      CHECK(enc.count == -1);
      rsh_encoder_step(&enc, 1, 0);   // 01 -> 10 invalid
      CHECK(enc.count == -1);
      // Same state repeat -> koi count nahi.
      rsh_encoder_step(&enc, 1, 0);
      rsh_encoder_step(&enc, 1, 0);
      CHECK(enc.count == -1);
      // Phase F WS5: reversal-undo debounce — foran ulta transition
      // pichla step cancel karta hai (bounce jora net 0, glitch gina jata hai).
      rsh_encoder_init(&enc);
      rsh_encoder_step(&enc, 0, 0);   // prev=00
      rsh_encoder_step(&enc, 1, 0);   // +1 -> 1
      CHECK(enc.count == 1);
      rsh_encoder_step(&enc, 0, 0);   // foran wapas -> undo, 0
      CHECK(enc.count == 0);
      CHECK(enc.glitches == 1);
      // Us ke baad asal motion phir se count hoti hai.
      rsh_encoder_step(&enc, 0, 1);   // -1 -> -1
      CHECK(enc.count == -1);
      CHECK(enc.glitches == 1);
      // Genuine reversal: 4 aage (+4) phir 4 peeche = wapas 0. Pehla
      // reverse step pichle forward step se jora banata hai (net 0),
      // baaqi 3 steps + undo = -4 — kul 0 (drift test ka chhota version).
      rsh_encoder_init(&enc);
      rsh_encoder_step(&enc, 0, 0);
      rsh_encoder_step(&enc, 1, 0); rsh_encoder_step(&enc, 1, 1);
      rsh_encoder_step(&enc, 0, 1); rsh_encoder_step(&enc, 0, 0);
      CHECK(enc.count == 4);
      rsh_encoder_step(&enc, 0, 1); rsh_encoder_step(&enc, 1, 1);
      rsh_encoder_step(&enc, 1, 0); rsh_encoder_step(&enc, 0, 0);
      CHECK(enc.count == 0);
      // i16 wrap-aware delta.
      CHECK(rsh_encoder_delta(-32768, 32767) == 1);
      CHECK(rsh_encoder_delta(1000, -250) == 1250);
      CHECK(rsh_encoder_delta(-250, 1000) == -1250);
      CHECK(rsh_encoder_delta(5, 5) == 0);
      // Count khud i16 wrap karta hai (int16_t overflow defined wrap).
      enc.count = 32760;
      enc.prev = 0; enc.seen = 1;
      enc.lastDelta = 0; enc.prevPrev = 0;  // manual setup mukammal karo —
      // warna pichle test ka stale lastDelta pehle step ko undo kar deta
      // (value ittefaq se wahi aati, lekin wajah ghalat hoti).
      rsh_encoder_step(&enc, 1, 0);  // +1 -> 32761
      CHECK(enc.count == 32761);
      for (int i = 0; i < 10; i++) {
        rsh_encoder_step(&enc, 1, 1);
        rsh_encoder_step(&enc, 0, 1);
        rsh_encoder_step(&enc, 0, 0);
        rsh_encoder_step(&enc, 1, 0);
      }
      // 32761 + 40 = 32801 -> wrap -> -32735.
      CHECK(enc.count == -32735);
      CHECK(rsh_encoder_delta(enc.count, 32761) == 40);
    }

    // --- FW-12: LED patterns + R-L9 ---
    {
      RshLedPattern cfg;
      // R-L9: direct rig mein sirf GPIO21; expanders mein 21/22 nahi.
      CHECK(rsh_led_test_allowed(21, 0) == 1);
      CHECK(rsh_led_test_allowed(4, 0) == 0);
      CHECK(rsh_led_test_allowed(21, 1) == 0);
      CHECK(rsh_led_test_allowed(22, 1) == 0);
      CHECK(rsh_led_test_allowed(4, 1) == 1);
      CHECK(rsh_led_test_allowed(2, 1) == 0);   // status LED kabhi nahi
      CHECK(rsh_led_test_allowed(34, 0) == 0);  // input-only pin, no output
      // Validate.
      cfg = {RSH_LED_SOLID, 255, 0, 0, 1000, 0, 0};
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_OK);
      CHECK(rsh_led_pattern_validate(&cfg, 4, 0, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 0) == RSH_CTRL_UNSUPPORTED);
      cfg.patternId = 5;
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
      cfg.patternId = RSH_LED_BLINK; cfg.periodMs = 50;
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
      cfg.periodMs = 100;
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_OK);
      cfg.reserved = 1;
      CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
      CHECK(rsh_led_pattern_validate(NULL, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
      // Hue wheel.
      uint8_t r, g, b;
      rsh_led_hue_rgb(0, &r, &g, &b);
      CHECK(r == 255 && g == 0 && b == 0);   // red
      rsh_led_hue_rgb(85, &r, &g, &b);
      CHECK(r < 10 && g == 255 && b == 0);   // green
      rsh_led_hue_rgb(170, &r, &g, &b);
      CHECK(r == 0 && g < 10 && b == 255);   // blue
      // BLINK: period 1000, repeats 2 -> 2000 ms; on/off period/2.
      RshLedPatternState st;
      cfg = {RSH_LED_BLINK, 255, 128, 0, 1000, 2, 0};
      rsh_led_pattern_start(&st, &cfg, 0);
      CHECK(rsh_led_pattern_done(&st, 1999) == 0);
      CHECK(rsh_led_pattern_done(&st, 2000) == 1);
      rsh_led_pattern_pixel(&st, 0, 0, 4, &r, &g, &b);
      CHECK(r == 255 && g == 128 && b == 0);   // on
      rsh_led_pattern_pixel(&st, 500, 0, 4, &r, &g, &b);
      CHECK(r == 0 && g == 0 && b == 0);        // off half
      rsh_led_pattern_pixel(&st, 1000, 0, 4, &r, &g, &b);
      CHECK(r == 255 && g == 128 && b == 0);   // doosra period on
      rsh_led_pattern_pixel(&st, 2000, 0, 4, &r, &g, &b);
      CHECK(r == 0 && g == 0 && b == 0);        // khatam — off
      // SOLID repeats 0 -> 5 s.
      cfg = {RSH_LED_SOLID, 0, 255, 0, 0, 0, 0};
      rsh_led_pattern_start(&st, &cfg, 1000);
      rsh_led_pattern_pixel(&st, 5999, 0, 1, &r, &g, &b);
      CHECK(r == 0 && g == 255 && b == 0);
      CHECK(rsh_led_pattern_done(&st, 5999) == 0);
      CHECK(rsh_led_pattern_done(&st, 6000) == 1);
      // CHASE: 4 LEDs, period 1000 -> har 250 ms agla pixel.
      cfg = {RSH_LED_CHASE, 0, 0, 255, 1000, 1, 0};
      rsh_led_pattern_start(&st, &cfg, 0);
      for (int i = 0; i < 4; i++) {
        rsh_led_pattern_pixel(&st, (uint32_t)(i * 250), (uint8_t)i, 4, &r, &g, &b);
        CHECK(b == 255);
        rsh_led_pattern_pixel(&st, (uint32_t)(i * 250), (uint8_t)((i + 1) % 4), 4, &r, &g, &b);
        CHECK(b == 0);   // sirf ek pixel lit
      }
      // RAINBOW: pixel 0 aur pixel 2 ke hue mukhtalif (offset).
      cfg = {RSH_LED_RAINBOW, 0, 0, 0, 1000, 1, 0};
      rsh_led_pattern_start(&st, &cfg, 0);
      uint8_t r2, g2, b2;
      rsh_led_pattern_pixel(&st, 0, 0, 4, &r, &g, &b);
      rsh_led_pattern_pixel(&st, 0, 2, 4, &r2, &g2, &b2);
      CHECK(!(r == r2 && g == g2 && b == b2));
      // OFF: foran done, pixel hamesha off.
      cfg = {RSH_LED_OFF, 0, 0, 0, 0, 0, 0};
      rsh_led_pattern_start(&st, &cfg, 0);
      CHECK(rsh_led_pattern_done(&st, 0) == 1);
      rsh_led_pattern_pixel(&st, 0, 0, 4, &r, &g, &b);
      CHECK(r == 0 && g == 0 && b == 0);
    }
  }

  // 12) FW-3 safe mode + boot counter (Workstream 1).
  {
    // --- Boot counter: kamyab boot counters reset karta hai ---
    rsh_boot_counter_t c;
    rsh_boot_counter_init(&c);
    rsh_boot_begin(&c);
    CHECK(c.total_boots == 1);
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_CURRENT);
    CHECK(rsh_boot_tick(&c, 0, 14000, 0) == RSH_BOOT_WAITING);  // 14 s: abhi wait
    CHECK(rsh_boot_tick(&c, 1, 14000, 0) == RSH_BOOT_SUCCESS);  // AP waqt par up
    CHECK(c.consecutive_failures == 0);
    CHECK(rsh_boot_tick(&c, 1, 20000, 0) == RSH_BOOT_WAITING);  // natija darj ho chuka
    // --- Boot fail: deadline (15 s) par ek dafa gin'a jata hai ---
    rsh_boot_counter_init(&c);
    rsh_boot_begin(&c);
    CHECK(rsh_boot_tick(&c, 0, 14999, 0) == RSH_BOOT_WAITING);
    CHECK(rsh_boot_tick(&c, 0, 15000, 0) == RSH_BOOT_FAILED);   // exactly 15 s
    CHECK(c.consecutive_failures == 1);
    CHECK(rsh_boot_tick(&c, 0, 20000, 0) == RSH_BOOT_WAITING);  // dobara nahi gin'a
    CHECK(c.consecutive_failures == 1);
    // --- 3 lagatar fail -> last-known-good; LKG valid na ho to factory ---
    rsh_boot_counter_init(&c);
    for (int boot = 0; boot < 3; boot++) {
      rsh_boot_begin(&c);
      CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_CURRENT);
      CHECK(rsh_boot_tick(&c, 0, (uint32_t)(boot * 100000 + 15000),
                          (uint32_t)(boot * 100000)) == RSH_BOOT_FAILED);
    }
    CHECK(c.consecutive_failures == 3);
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_LAST_KNOWN_GOOD);
    CHECK(rsh_boot_decide_source(&c, 0) == RSH_BOOT_SRC_FACTORY);
    // LKG boot kamyab ho to counters reset
    rsh_boot_begin(&c);
    c.using_lkg = 1;
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_LAST_KNOWN_GOOD);
    CHECK(rsh_boot_tick(&c, 1, 5000, 300000) == RSH_BOOT_SUCCESS);
    CHECK(c.consecutive_failures == 0 && c.lkg_consecutive_failures == 0);
    // LKG se 3 fail -> factory (factory source par bhi fail gin'a jata hai)
    rsh_boot_counter_init(&c);
    c.consecutive_failures = 3;
    for (int boot = 0; boot < 3; boot++) {
      rsh_boot_begin(&c);
      c.using_lkg = 1;
      CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_LAST_KNOWN_GOOD);
      CHECK(rsh_boot_tick(&c, 0, (uint32_t)(boot * 100000 + 15000),
                          (uint32_t)(boot * 100000)) == RSH_BOOT_FAILED);
    }
    CHECK(c.lkg_consecutive_failures == 3 && c.consecutive_failures == 3);
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_FACTORY);
    // LKG fail current counter nahi barhata
    rsh_boot_counter_init(&c);
    rsh_boot_begin(&c);
    c.using_lkg = 1;
    CHECK(rsh_boot_tick(&c, 0, 15000, 0) == RSH_BOOT_FAILED);
    CHECK(c.lkg_consecutive_failures == 1 && c.consecutive_failures == 0);
    // --- millis wrap-safe: boot_start wrap ke qareeb ---
    rsh_boot_counter_init(&c);
    rsh_boot_begin(&c);
    uint32_t wrap_start = 0xFFFFFF00u;
    CHECK(rsh_boot_tick(&c, 0, wrap_start + 14999, wrap_start) == RSH_BOOT_WAITING);
    CHECK(rsh_boot_tick(&c, 0, wrap_start + 15000, wrap_start) == RSH_BOOT_FAILED);

    // --- BOOT-hold: < 5 s kuch nahi; 5 s safe mode; 10 s key-reset-armed ---
    rsh_boothold_t h;
    rsh_boothold_init(&h);
    CHECK(rsh_boothold_update(&h, 1, 0) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 4999) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 5000) == RSH_HOLD_SAFE_MODE);  // exactly 5 s
    CHECK(rsh_boothold_update(&h, 1, 6000) == RSH_HOLD_NONE);       // ek dafa hi fire
    CHECK(rsh_boothold_update(&h, 1, 9999) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 10000) == RSH_HOLD_KEY_RESET_ARMED);
    CHECK(rsh_boothold_update(&h, 1, 20000) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 0, 20001) == RSH_HOLD_NONE);      // release = reset
    // Chhota press (bounce / < 5 s) kuch nahi karta; dobara hold naye sire se
    rsh_boothold_init(&h);
    CHECK(rsh_boothold_update(&h, 1, 0) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 2000) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 0, 2001) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 3000) == RSH_HOLD_NONE);       // hold dobara 0 se
    CHECK(rsh_boothold_update(&h, 1, 7999) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 8000) == RSH_HOLD_SAFE_MODE);  // 3000+5000
    // millis wrap ke paar hold bhi sahi gin'a jata hai
    rsh_boothold_init(&h);
    CHECK(rsh_boothold_update(&h, 1, 0xFFFFFF00u) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&h, 1, 0xFFFFFF00u + 5000) == RSH_HOLD_SAFE_MODE);

    // --- Safe-mode state: config block + armed window expiry ---
    rsh_safe_mode_t sm;
    rsh_safe_mode_init(&sm);
    CHECK(rsh_safe_mode_blocks_config(&sm) == 0);
    CHECK(rsh_safe_mode_tick(&sm, 1000) == 0);
    rsh_safe_mode_enter(&sm, 0, 1000);   // safe mode (5 s hold)
    CHECK(rsh_safe_mode_blocks_config(&sm) == 1);
    CHECK(rsh_safe_mode_tick(&sm, 1000000) == 0);  // safe mode ki koi expiry nahi
    rsh_safe_mode_init(&sm);
    rsh_safe_mode_enter(&sm, 1, 5000);   // key-reset-armed (10 s hold)
    CHECK(rsh_safe_mode_blocks_config(&sm) == 1);
    CHECK(rsh_safe_mode_tick(&sm, 5000 + 119999) == 0);
    CHECK(rsh_safe_mode_tick(&sm, 5000 + 120000) == 1);  // window khatam -> reboot
    // HELLO flags bit2 safeMode hai (§2.2) — constant composition check
    CHECK(RSH_V2_HELLO_SAFE_MODE == 0x04);
    CHECK((RSH_V2_HELLO_SAFE_MODE | RSH_V2_HELLO_KEY_RESET) == 0x06);
  }

  // 13) FW-4 config transaction (Workstream 1).
  {
    // --- CRC32 check values (ISO-HDLC) + independent bitwise cross-check ---
    CHECK(rsh_crc32((const uint8_t *)"123456789", 9) == 0xCBF43926u);
    CHECK(rsh_crc32((const uint8_t *)"", 0) == 0x00000000u);
    {
      // Bitwise reference (table-driven se alag tareeqa), 200 random buffers.
      for (int t = 0; t < 200; t++) {
        uint8_t buf[64];
        size_t n = rng() % 65;
        for (size_t i = 0; i < n; i++) buf[i] = (uint8_t)rng();
        uint32_t ref = 0xFFFFFFFFu;
        for (size_t i = 0; i < n; i++) {
          ref ^= buf[i];
          for (int b = 0; b < 8; b++) ref = (ref & 1) ? (0xEDB88320u ^ (ref >> 1)) : (ref >> 1);
        }
        ref ^= 0xFFFFFFFFu;
        CHECK(rsh_crc32(buf, n) == ref);
      }
    }

    // --- Factory blob: poora pin map (§3.6 slot scheme, Phase G C3),
    // config.h ke PIN_* se match kare ---
    uint8_t factory[256];
    size_t flen = rsh_cfg_factory_blob(factory);
    CHECK(flen == 188);  // 8 header + 11*16 records + 4 trailer
    CHECK(rsh_cfg_blob_validate(factory, flen, NULL, NULL) == RSH_CTRL_OK);
    CHECK(factory[0] == 'R' && factory[1] == 'S' && factory[2] == 'H' && factory[3] == 'G');
    CHECK(factory[4] == RSH_CFG_SCHEMA && factory[5] == 11);
    CHECK(factory[8 + 0 * 16 + 3] == PIN_STEER);          // AXIS slot 0
    CHECK(factory[8 + 1 * 16 + 3] == PIN_ACCEL_ADC);      // AXIS slot 1
    CHECK(factory[8 + 2 * 16 + 3] == PIN_BRAKE_ADC);      // AXIS slot 2
    CHECK(factory[8 + 3 * 16 + 3] == PIN_HANDBRAKE_ADC);  // AXIS slot 4
    CHECK(factory[8 + 4 * 16 + 3] == PIN_BATTERY_ADC);    // BATTERY slot 0
    CHECK(factory[8 + 5 * 16 + 3] == PIN_GEAR_UP);        // BUTTON slot 0
    CHECK(factory[8 + 6 * 16 + 3] == PIN_GEAR_DOWN);      // BUTTON slot 1
    CHECK(factory[8 + 7 * 16 + 3] == PIN_HORN_BTN);       // BUTTON slot 2
    CHECK(factory[8 + 8 * 16 + 3] == PIN_ENC1_A && factory[8 + 8 * 16 + 4] == PIN_ENC1_B);
    CHECK(factory[8 + 9 * 16 + 3] == PIN_ENC2_A && factory[8 + 9 * 16 + 4] == PIN_ENC2_B);
    CHECK(factory[8 + 10 * 16 + 3] == PIN_LED_STRIP);     // LED_STRIP slot 0

    // --- Blob validation negatives (har ghalti ka sahi status + errCode) ---
    {
      uint8_t errs[8];
      uint8_t nerr = 0;
      uint8_t bad[256];
      // CRC32 trailer kharab -> BAD_CRC, err 4
      memcpy(bad, factory, flen);
      bad[flen - 1] ^= 0xFF;
      CHECK(rsh_cfg_blob_validate(bad, flen, errs, &nerr) == RSH_CTRL_BAD_CRC);
      CHECK(nerr == 1 && errs[0] == RSH_CFG_ERR_CRC_MISMATCH);
      // Magic kharab -> UNSUPPORTED, err 3
      memcpy(bad, factory, flen);
      bad[0] = 'X';
      CHECK(rsh_cfg_blob_validate(bad, flen, errs, &nerr) == RSH_CTRL_UNSUPPORTED);
      CHECK(errs[0] == RSH_CFG_ERR_UNKNOWN_TYPE);
      // Schema kharab -> UNSUPPORTED, err 3
      memcpy(bad, factory, flen);
      bad[4] = 3;
      rsh_put_u32(bad + flen - 4, rsh_crc32(bad, flen - 4));  // CRC dobara sahi
      CHECK(rsh_cfg_blob_validate(bad, flen, errs, &nerr) == RSH_CTRL_UNSUPPORTED);
      // Size mismatch (moduleCount 11 lekin len chhoti) -> TOO_BIG, err 5
      CHECK(rsh_cfg_blob_validate(factory, flen - 16, errs, &nerr) == RSH_CTRL_TOO_BIG);
      CHECK(errs[0] == RSH_CFG_ERR_SIZE_MISMATCH);
      CHECK(rsh_cfg_blob_validate(factory, 8, errs, &nerr) == RSH_CTRL_TOO_BIG);
      // Unknown module type (CRC dobara lagao taake sirf type fail ho)
      uint8_t rec[16];
      rsh_cfg_record(rec, 13, 0, 0, 21, 0xFF, 0xFF, NULL);  // type 13 maujood nahi
      // (Phase G WS4: type 12 ab AXIS_AS5600 hai — unknown type ab 13 se shuru)
      uint8_t one[64];
      size_t olen = rsh_cfg_blob_build(one, rec, 1, 1);
      CHECK(olen == 28 && rsh_cfg_blob_validate(one, olen, errs, &nerr) == RSH_CTRL_UNSUPPORTED);
      CHECK(errs[0] == RSH_CFG_ERR_UNKNOWN_TYPE);
      // Duplicate pin (do records, ek hi GPIO) -> PIN_CONFLICT, err 1
      uint8_t recs[32];
      rsh_cfg_record(recs + 0, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
      rsh_cfg_record(recs + 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 34, 0xFF, 0xFF, NULL);
      uint8_t dup[64];
      size_t dlen = rsh_cfg_blob_build(dup, recs, 2, 1);
      CHECK(rsh_cfg_blob_validate(dup, dlen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_PIN_CONFLICT);
      // Pin 40 (0..39 ke bahar) -> PIN_CONFLICT, err 2
      rsh_cfg_record(recs + 0, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 40, 0xFF, 0xFF, NULL);
      rsh_cfg_record(recs + 16, RSH_CFG_TYPE_BUTTON, 0, 0, 25, 0xFF, 0xFF, NULL);
      dlen = rsh_cfg_blob_build(dup, recs, 2, 1);
      CHECK(rsh_cfg_blob_validate(dup, dlen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // 9 axes (> 8) -> PIN_CONFLICT, err 2 (ADS1115 axes ko pin nahi
      // chahiye, is liye duplicate-pin ka masla nahi — sirf axes count)
      uint8_t many[9 * 16];
      for (int i = 0; i < 9; i++)
        rsh_cfg_record(many + i * 16, RSH_CFG_TYPE_AXIS_ADS1115, (uint8_t)i, 0,
                       0xFF, 0xFF, 0xFF, NULL);
      uint8_t manyb[256];
      size_t mlen = rsh_cfg_blob_build(manyb, many, 9, 1);
      CHECK(rsh_cfg_blob_validate(manyb, mlen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // 5 encoders (> 4) -> PIN_CONFLICT, err 2
      uint8_t enc[5 * 16];
      for (int i = 0; i < 5; i++)
        rsh_cfg_record(enc + i * 16, RSH_CFG_TYPE_ENCODER, (uint8_t)i, 0,
                       (uint8_t)(25 + i), (uint8_t)(30 + i), 0xFF, NULL);
      uint8_t encb[256];
      size_t elen = rsh_cfg_blob_build(encb, enc, 5, 1);
      CHECK(rsh_cfg_blob_validate(encb, elen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
      // 65 buttons (N1: wire slot nahi) -> UNSUPPORTED, err 3
      // (65 BUTTON records — 65*16+12 = 1052 B, limit ke andar)
      static uint8_t btnrec[65 * 16];
      for (int i = 0; i < 65; i++)
        rsh_cfg_record(btnrec + i * 16, RSH_CFG_TYPE_BUTTON, (uint8_t)i, 0,
                       0xFF, 0xFF, 0xFF, NULL);
      static uint8_t btnb[2048];
      size_t blen = rsh_cfg_blob_build(btnb, btnrec, 65, 1);
      CHECK(blen == 1052);
      CHECK(rsh_cfg_blob_validate(btnb, blen, errs, &nerr) == RSH_CTRL_UNSUPPORTED);
      CHECK(errs[0] == RSH_CFG_ERR_UNKNOWN_TYPE);
    }

    // --- M1 (final polish): PIN-RULES port — blob validator pin rules ---
    // Accept: factory blob (upar) + PC default pin-map blob
    // (PinMapBoardPush.BuildBlob defaults) + direct full-rig blob
    // (pin_rules_ref.py make_full_rig + LED 21) — teeno OK hone chahiye.
    // Reject: ADC2 axis, barred pins (strapping/flash/UART/reserved/
    // nonexistent), input-only digital/LED, I2C-reserved 21/22, 2 batteries,
    // 2 LED strips, I2C kind par GPIO pin.
    {
      uint8_t errs[8];
      uint8_t nerr = 0;
      uint8_t blob[512];
      auto v = [&](const uint8_t *recs, uint8_t n) {
        size_t l = rsh_cfg_blob_build(blob, recs, n, 1);
        nerr = 0;
        return rsh_cfg_blob_validate(blob, l, errs, &nerr);
      };
      // PC default pin-map blob (PinMapBoardPush defaults): steering 34,
      // accel 35, brake 36, handbrake 32, battery 39, gear-up 27,
      // gear-down 33, horn 4, enc1 23/19, enc2 16/17, LED 21.
      uint8_t pc[11 * 16];
      rsh_cfg_record(pc + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 4, 0, 32, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 4 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 5 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 27, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 6 * 16, RSH_CFG_TYPE_BUTTON, 1, 0, 33, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 7 * 16, RSH_CFG_TYPE_BUTTON, 2, 0, 4, 0xFF, 0xFF, NULL);
      rsh_cfg_record(pc + 8 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
      rsh_cfg_record(pc + 9 * 16, RSH_CFG_TYPE_ENCODER, 1, 0, 16, 17, 0xFF, NULL);
      rsh_cfg_record(pc + 10 * 16, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
      CHECK(v(pc, 11) == RSH_CTRL_OK);
      // Direct full rig (pin_rules_ref.py): 5 axes + battery + 7 buttons
      // + 2 encoders + LED 21.
      uint8_t rig[16 * 16];
      rsh_cfg_record(rig + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 32, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 33, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 34, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 35, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 4 * 16, RSH_CFG_TYPE_AXIS_ADC, 4, 0, 36, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 5 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 6 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 4, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 7 * 16, RSH_CFG_TYPE_BUTTON, 1, 0, 13, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 8 * 16, RSH_CFG_TYPE_BUTTON, 2, 0, 14, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 9 * 16, RSH_CFG_TYPE_BUTTON, 3, 0, 16, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 10 * 16, RSH_CFG_TYPE_BUTTON, 4, 0, 17, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 11 * 16, RSH_CFG_TYPE_BUTTON, 5, 0, 18, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 12 * 16, RSH_CFG_TYPE_BUTTON, 6, 0, 19, 0xFF, 0xFF, NULL);
      rsh_cfg_record(rig + 13 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 25, 0xFF, NULL);
      rsh_cfg_record(rig + 14 * 16, RSH_CFG_TYPE_ENCODER, 1, 0, 26, 27, 0xFF, NULL);
      rsh_cfg_record(rig + 15 * 16, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
      CHECK(v(rig, 16) == RSH_CTRL_OK);
      // GPIO5 (caution, boot log) allowed hai — PC + ref dono allow karte hain.
      uint8_t one[16];
      rsh_cfg_record(one, RSH_CFG_TYPE_BUTTON, 0, 0, 5, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_OK);
      // ADC2 axis (GPIO25) reject — ADC1-only rule.
      rsh_cfg_record(one, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 25, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // Barred pins (BUTTON par): strapping 0/12/15, UART 1/3, flash 6,
      // reserved 2, nonexistent 20/37 — sab err 2.
      const uint8_t barred[] = {0, 1, 2, 3, 6, 12, 15, 20, 37};
      for (size_t bi = 0; bi < sizeof(barred); bi++) {
        rsh_cfg_record(one, RSH_CFG_TYPE_BUTTON, 0, 0, barred[bi], 0xFF, 0xFF, NULL);
        CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
        CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      }
      // Input-only pin par digital: encoder 34/35, button 34 — pull-up
      // wahan maujood hi nahi (INPUT_PULLUP), is liye reject.
      rsh_cfg_record(one, RSH_CFG_TYPE_ENCODER, 0, 0, 34, 35, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      rsh_cfg_record(one, RSH_CFG_TYPE_BUTTON, 0, 0, 34, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // Input-only pin par LED (digital-out chahiye) reject.
      rsh_cfg_record(one, RSH_CFG_TYPE_LED_STRIP, 0, 0, 39, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // I2C kinds hon to GPIO21/22 I2C hain: LED 21 + expander reject,
      // LED 4 + expander OK; AS5600 (I2C) + LED 21 bhi reject.
      uint8_t two[2 * 16];
      rsh_cfg_record(two + 0, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
      rsh_cfg_record(two + 16, RSH_CFG_TYPE_EXP_MCP23017, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
      CHECK(v(two, 2) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_PIN_CONFLICT);
      rsh_cfg_record(two + 0, RSH_CFG_TYPE_LED_STRIP, 0, 0, 4, 0xFF, 0xFF, NULL);
      CHECK(v(two, 2) == RSH_CTRL_OK);
      rsh_cfg_record(two + 0, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
      rsh_cfg_record(two + 16, RSH_CFG_TYPE_AXIS_AS5600, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
      CHECK(v(two, 2) == RSH_CTRL_PIN_CONFLICT);
      // I2C kind par GPIO pin reject (ADS1115 pinA 32, expander pinA 4).
      rsh_cfg_record(one, RSH_CFG_TYPE_AXIS_ADS1115, 0, 0, 32, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      rsh_cfg_record(one, RSH_CFG_TYPE_AXIS_ADS1115, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
      CHECK(v(one, 1) == RSH_CTRL_OK);
      // 2 batteries / 2 LED strips (pin wale) reject.
      rsh_cfg_record(two + 0, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
      rsh_cfg_record(two + 16, RSH_CFG_TYPE_BATTERY, 1, 0, 36, 0xFF, 0xFF, NULL);
      CHECK(v(two, 2) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      rsh_cfg_record(two + 0, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
      rsh_cfg_record(two + 16, RSH_CFG_TYPE_LED_STRIP, 1, 0, 4, 0xFF, 0xFF, NULL);
      CHECK(v(two, 2) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
      // H-pattern gear pin input-only ho to reject (pull-up rule).
      uint8_t hp[10] = {1, 17, 18, 19, 0, 0, 0, 0, 0, 0};
      rsh_cfg_record(one, RSH_CFG_TYPE_SHIFTER, 0, 0, 34, 14, 16, hp);
      CHECK(v(one, 1) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
    }

    // --- Staging: out-of-order chunks + idempotent resend + naya session ---
    {
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      CHECK(t.current_len == flen && t.cfg_version == 1);
      // Nayi config: factory jaisi lekin version 2 (alag blob)
      uint8_t newblob[256];
      size_t nlen = rsh_cfg_factory_blob(newblob);
      // version byte badlo + CRC dobara lagao
      rsh_put_u16(newblob + 6, 2);
      rsh_put_u32(newblob + nlen - 4, rsh_crc32(newblob, nlen - 4));
      CHECK(rsh_cfg_blob_validate(newblob, nlen, NULL, NULL) == RSH_CTRL_OK);
      uint16_t sb = 0, tot = 0;
      // Pehla chunk offset 0 lazmi hai (session shuru); baaqi out-of-order.
      uint16_t c0 = 31, c1 = 31, c2 = 31, c3 = (uint16_t)(nlen - 93);
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, c0,
                                rsh_crc16_ccitt(newblob, c0), newblob, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(sb == 31 && tot == nlen && t.state == RSH_TXN_STAGING);
      // Chunk 3 (aakhri) pehle, phir chunk 2, phir chunk 1 — out-of-order ok
      CHECK(rsh_cfg_stage_chunk(&t, 93, (uint16_t)nlen, c3,
                                rsh_crc16_ccitt(newblob + 93, c3), newblob + 93, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(sb == 31 + c3 && t.state == RSH_TXN_STAGING);
      CHECK(rsh_cfg_stage_chunk(&t, 62, (uint16_t)nlen, c2,
                                rsh_crc16_ccitt(newblob + 62, c2), newblob + 62, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(rsh_cfg_stage_chunk(&t, 31, (uint16_t)nlen, c1,
                                rsh_crc16_ccitt(newblob + 31, c1), newblob + 31, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(sb == nlen && t.state == RSH_TXN_STAGED);
      CHECK(memcmp(t.staged, newblob, nlen) == 0);
      // Dobara bheja gaya chunk idempotent hai (gin'a nahi jata)
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, c0,
                                rsh_crc16_ccitt(newblob, c0), newblob, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(sb == nlen && t.state == RSH_TXN_STAGED);
      // Naya session (offset 0, alag total) purana staged discard karta hai
      uint8_t small[64];
      uint8_t srec[16];
      rsh_cfg_record(srec, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
      size_t slen = rsh_cfg_blob_build(small, srec, 1, 5);
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)slen, (uint16_t)slen,
                                rsh_crc16_ccitt(small, (uint16_t)slen), small, &sb, &tot)
            == RSH_CTRL_OK);
      CHECK(sb == slen && tot == slen && t.state == RSH_TXN_STAGED);
      CHECK(memcmp(t.staged, small, slen) == 0);
      // REVERT staging discard karta hai, current waisa ka waisa
      CHECK(rsh_cfg_revert(&t) == RSH_CTRL_OK);
      CHECK(t.state == RSH_TXN_IDLE && t.current_len == flen);
      CHECK(memcmp(t.current, factory, flen) == 0);
      CHECK(rsh_cfg_revert(&t) == RSH_CTRL_OK);  // idle par no-op OK
    }
    // --- Staging negatives ---
    {
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      uint16_t sb = 0, tot = 0;
      uint8_t data[64] = {0};
      // chunk > 1024
      CHECK(rsh_cfg_stage_chunk(&t, 0, 2048, 1025, 0, data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      // total > 2048
      CHECK(rsh_cfg_stage_chunk(&t, 0, 2049, 64, 0, data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      // total 0 / chunk 0
      CHECK(rsh_cfg_stage_chunk(&t, 0, 0, 64, 0, data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      CHECK(rsh_cfg_stage_chunk(&t, 0, 64, 0, 0, data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      // offset + len > total
      CHECK(rsh_cfg_stage_chunk(&t, 100, 124, 64, 0, data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      // pehla chunk offset 0 nahi (session shuru nahi hui) — chunk CRC
      // sahi diya hai taake check offset rule tak pahunche, CRC par na ruke
      CHECK(rsh_cfg_stage_chunk(&t, 31, 124, 31, rsh_crc16_ccitt(data, 31),
                                data, &sb, &tot)
            == RSH_CTRL_TOO_BIG);
      // chunk CRC ghalat -> BAD_CRC (state nahi badalti)
      CHECK(rsh_cfg_stage_chunk(&t, 0, 124, 31, 0x1234, factory, &sb, &tot)
            == RSH_CTRL_BAD_CRC);
      CHECK(t.state == RSH_TXN_IDLE && t.received_count == 0);
      // COMMIT: kuch staged nahi -> UNSUPPORTED; aadha staged -> TOO_BIG
      CHECK(rsh_cfg_commit(&t, 1000, NULL, NULL) == RSH_CTRL_UNSUPPORTED);
      CHECK(rsh_cfg_stage_chunk(&t, 0, 124, 31,
                                rsh_crc16_ccitt(factory, 31), factory, &sb, &tot)
            == RSH_CTRL_OK);
      uint8_t errs[8];
      uint8_t nerr = 0;
      CHECK(rsh_cfg_commit(&t, 1000, errs, &nerr) == RSH_CTRL_TOO_BIG);
      CHECK(nerr == 1 && errs[0] == RSH_CFG_ERR_SIZE_MISMATCH);
      // CONFIRM: kuch committed nahi -> UNSUPPORTED
      CHECK(rsh_cfg_confirm(&t, 1000) == RSH_CTRL_UNSUPPORTED);
    }
    // --- COMMIT + CONFIRM (happy path) + deadline hamesha 30 s (D28) ---
    {
      CHECK(RSH_CFG_CONFIRM_TIMEOUT_MS == 30000u);
      CHECK(RSH_CFG_CONFIRM_DEADLINE_SEC == 30);
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      uint8_t newblob[256];
      size_t nlen = rsh_cfg_factory_blob(newblob);
      rsh_put_u16(newblob + 6, 2);
      rsh_put_u32(newblob + nlen - 4, rsh_crc32(newblob, nlen - 4));
      uint16_t sb = 0, tot = 0;
      // 64-byte chunks se stage karo
      for (size_t off = 0; off < nlen; off += 64) {
        uint16_t cl = (uint16_t)((nlen - off > 64) ? 64 : (nlen - off));
        CHECK(rsh_cfg_stage_chunk(&t, (uint16_t)off, (uint16_t)nlen, cl,
                                  rsh_crc16_ccitt(newblob + off, cl),
                                  newblob + off, &sb, &tot) == RSH_CTRL_OK);
      }
      CHECK(t.state == RSH_TXN_STAGED);
      CHECK(rsh_cfg_commit(&t, 1000, NULL, NULL) == RSH_CTRL_OK);
      CHECK(t.state == RSH_TXN_COMMITTED);
      CHECK(t.confirm_deadline_ms == 1000 + 30000);
      CHECK(t.current_len == nlen && t.cfg_version == 2);   // live apply ho gayi
      CHECK(memcmp(t.current, newblob, nlen) == 0);
      CHECK(t.has_previous == 1 && t.previous_len == flen);  // rollback copy
      CHECK(memcmp(t.previous, factory, flen) == 0);
      // Dobara COMMIT (retry) deadline nahi barhata — timer dobara shuru nahi
      CHECK(rsh_cfg_commit(&t, 5000, NULL, NULL) == RSH_CTRL_OK);
      CHECK(t.confirm_deadline_ms == 1000 + 30000);
      // Stage COMMITTED state mein BUSY hai (confirm window mein conflicting op)
      uint8_t data[16] = {0};
      CHECK(rsh_cfg_stage_chunk(&t, 0, 124, 16, rsh_crc16_ccitt(data, 16),
                                data, &sb, &tot) == RSH_CTRL_BUSY);
      // Tick deadline se pehle kuch nahi karta
      CHECK(rsh_cfg_tick(&t, 1000 + 29999) == 0);
      // Waqt par CONFIRM -> permanent
      CHECK(rsh_cfg_confirm(&t, 1000 + 29999) == RSH_CTRL_OK);
      CHECK(t.state == RSH_TXN_IDLE && t.current_len == nlen);
      CHECK(memcmp(t.current, newblob, nlen) == 0);
      CHECK(t.has_previous == 0);
      // GET_CFG: current config se parho (clamp + out-of-range)
      uint8_t out[64];
      CHECK(rsh_cfg_get_chunk(&t, 0, 8, out) == 8);
      CHECK(memcmp(out, "RSHG", 4) == 0);
      CHECK(rsh_cfg_get_chunk(&t, (uint16_t)(nlen - 4), 64, out) == 4);
      CHECK(rsh_cfg_get_chunk(&t, (uint16_t)nlen, 8, out) == 0);
    }
    // --- Rollback on timeout (gate: confirm na ho to rollback) ---
    {
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      uint8_t newblob[256];
      size_t nlen = rsh_cfg_factory_blob(newblob);
      rsh_put_u16(newblob + 6, 7);
      rsh_put_u32(newblob + nlen - 4, rsh_crc32(newblob, nlen - 4));
      uint16_t sb = 0, tot = 0;
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(newblob, (uint16_t)nlen),
                                newblob, &sb, &tot) == RSH_CTRL_OK);
      CHECK(rsh_cfg_commit(&t, 1000, NULL, NULL) == RSH_CTRL_OK);
      // Deadline par exactly rollback (30 s poora)
      CHECK(rsh_cfg_tick(&t, 1000 + 30000) == 1);
      CHECK(t.state == RSH_TXN_IDLE && t.current_len == flen);
      CHECK(memcmp(t.current, factory, flen) == 0);   // purani config wapas
      CHECK(t.cfg_version == 1);
      CHECK(rsh_cfg_tick(&t, 1000 + 60000) == 0);     // dobara rollback nahi
      // Der se CONFIRM: pehle rollback, phir UNSUPPORTED
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(newblob, (uint16_t)nlen),
                                newblob, &sb, &tot) == RSH_CTRL_OK);
      CHECK(rsh_cfg_commit(&t, 50000, NULL, NULL) == RSH_CTRL_OK);
      CHECK(rsh_cfg_confirm(&t, 50000 + 30000) == RSH_CTRL_UNSUPPORTED);
      CHECK(t.state == RSH_TXN_IDLE && memcmp(t.current, factory, flen) == 0);
      // REVERT committed-unconfirmed ko bhi wapas lata hai
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(newblob, (uint16_t)nlen),
                                newblob, &sb, &tot) == RSH_CTRL_OK);
      CHECK(rsh_cfg_commit(&t, 90000, NULL, NULL) == RSH_CTRL_OK);
      CHECK(rsh_cfg_revert(&t) == RSH_CTRL_OK);
      CHECK(memcmp(t.current, factory, flen) == 0 && t.state == RSH_TXN_IDLE);
      // Wrap-safe deadline: commit wrap ke qareeb, tick wrap ke paar
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(newblob, (uint16_t)nlen),
                                newblob, &sb, &tot) == RSH_CTRL_OK);
      uint32_t wrap_commit = 0xFFFFF000u;
      CHECK(rsh_cfg_commit(&t, wrap_commit, NULL, NULL) == RSH_CTRL_OK);
      CHECK(rsh_cfg_tick(&t, wrap_commit + 29999) == 0);
      CHECK(rsh_cfg_tick(&t, wrap_commit + 30000) == 1);
      CHECK(memcmp(t.current, factory, flen) == 0);
    }
    // --- COMMIT par bad blob reject hota hai (bad config kabhi live nahi) ---
    {
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      // Blob jiska CRC32 trailer ghalat hai (chunk CRCs sahi — chunk layer ok)
      uint8_t badblob[256];
      size_t nlen = rsh_cfg_factory_blob(badblob);
      badblob[nlen - 1] ^= 0xFF;  // trailer kharab
      uint16_t sb = 0, tot = 0;
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(badblob, (uint16_t)nlen),
                                badblob, &sb, &tot) == RSH_CTRL_OK);
      CHECK(t.state == RSH_TXN_STAGED);  // staging CRC32 check nahi karti (§3.5)
      uint8_t errs[8];
      uint8_t nerr = 0;
      CHECK(rsh_cfg_commit(&t, 1000, errs, &nerr) == RSH_CTRL_BAD_CRC);
      CHECK(nerr == 1 && errs[0] == RSH_CFG_ERR_CRC_MISMATCH);
      CHECK(t.state == RSH_TXN_STAGED);   // fix ya REVERT ka mauqa rehta hai
      CHECK(memcmp(t.current, factory, flen) == 0);  // live config chhooi nahi
      // Pin conflict wala blob bhi COMMIT par reject hota hai
      uint8_t recs[32];
      rsh_cfg_record(recs + 0, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
      rsh_cfg_record(recs + 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 34, 0xFF, 0xFF, NULL);
      uint8_t dup[64];
      size_t dlen = rsh_cfg_blob_build(dup, recs, 2, 3);
      CHECK(rsh_cfg_revert(&t) == RSH_CTRL_OK);
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)dlen, (uint16_t)dlen,
                                rsh_crc16_ccitt(dup, (uint16_t)dlen),
                                dup, &sb, &tot) == RSH_CTRL_OK);
      CHECK(rsh_cfg_commit(&t, 2000, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
      CHECK(errs[0] == RSH_CFG_ERR_PIN_CONFLICT);
      CHECK(memcmp(t.current, factory, flen) == 0);
    }
    // --- Safe mode mein stage + commit BUSY (config apply nahi hoti) ---
    {
      rsh_cfg_txn_t t;
      rsh_cfg_txn_init(&t);
      CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
      rsh_safe_mode_t sm;
      rsh_safe_mode_init(&sm);
      rsh_safe_mode_enter(&sm, 0, 1000);
      rsh_cfg_set_safe_mode(&t, rsh_safe_mode_blocks_config(&sm));
      uint16_t sb = 0, tot = 0;
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)flen, (uint16_t)flen,
                                rsh_crc16_ccitt(factory, (uint16_t)flen),
                                factory, &sb, &tot) == RSH_CTRL_BUSY);
      CHECK(t.state == RSH_TXN_IDLE);
      // Safe mode khatam (naye txn mein flag 0) -> stage phir chalta hai
      rsh_cfg_set_safe_mode(&t, 0);
      CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)flen, (uint16_t)flen,
                                rsh_crc16_ccitt(factory, (uint16_t)flen),
                                factory, &sb, &tot) == RSH_CTRL_OK);
    }
    // --- Boot recover: reboot ke doran confirm window mein commit -> rollback ---
    {
      // Pehle boot: factory current, nayi config commit (pending confirm)
      rsh_cfg_txn_t t1;
      rsh_cfg_txn_init(&t1);
      CHECK(rsh_cfg_txn_load_current(&t1, factory, flen) == RSH_CTRL_OK);
      uint8_t newblob[256];
      size_t nlen = rsh_cfg_factory_blob(newblob);
      rsh_put_u16(newblob + 6, 9);
      rsh_put_u32(newblob + nlen - 4, rsh_crc32(newblob, nlen - 4));
      uint16_t sb = 0, tot = 0;
      CHECK(rsh_cfg_stage_chunk(&t1, 0, (uint16_t)nlen, (uint16_t)nlen,
                                rsh_crc16_ccitt(newblob, (uint16_t)nlen),
                                newblob, &sb, &tot) == RSH_CTRL_OK);
      CHECK(rsh_cfg_commit(&t1, 1000, NULL, NULL) == RSH_CTRL_OK);
      // "Reboot": naya txn — NVS se current=nayi, previous=purani, pending=1
      rsh_cfg_txn_t t2;
      rsh_cfg_txn_init(&t2);
      CHECK(rsh_cfg_txn_load_current(&t2, t1.current, t1.current_len) == RSH_CTRL_OK);
      memcpy(t2.previous, t1.previous, t1.previous_len);
      t2.previous_len = t1.previous_len;
      t2.has_previous = 1;
      CHECK(rsh_cfg_boot_recover(&t2, 1) == 1);   // rollback hua
      CHECK(memcmp(t2.current, factory, flen) == 0);
      CHECK(t2.state == RSH_TXN_IDLE && t2.has_previous == 0);
      // Pending flag na ho to kuch nahi badalta
      rsh_cfg_txn_t t3;
      rsh_cfg_txn_init(&t3);
      CHECK(rsh_cfg_txn_load_current(&t3, newblob, nlen) == RSH_CTRL_OK);
      CHECK(rsh_cfg_boot_recover(&t3, 0) == 0);
      CHECK(memcmp(t3.current, newblob, nlen) == 0);
      // Pending ho lekin previous copy na ho (invariant tooti) -> 0, factory ka
      // faisla caller karta hai — yahan sirf ye ke crash/UB nahi hota
      rsh_cfg_txn_t t4;
      rsh_cfg_txn_init(&t4);
      CHECK(rsh_cfg_txn_load_current(&t4, newblob, nlen) == RSH_CTRL_OK);
      CHECK(rsh_cfg_boot_recover(&t4, 1) == 0);
      CHECK(memcmp(t4.current, newblob, nlen) == 0);
    }
    // --- Bad config -> 3 boot mein recover (boot counter + LKG, end-to-end) ---
    {
      rsh_boot_counter_t c;
      rsh_boot_counter_init(&c);
      // Boot 1-3: bad current config — AP 15 s mein up nahi hota
      for (int boot = 0; boot < 3; boot++) {
        rsh_boot_begin(&c);
        CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_CURRENT);
        CHECK(rsh_boot_tick(&c, 0, (uint32_t)(boot * 60000 + 15000),
                            (uint32_t)(boot * 60000)) == RSH_BOOT_FAILED);
      }
      // Boot 4: LKG source — AP up, board recover karta hai
      rsh_boot_begin(&c);
      CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_LAST_KNOWN_GOOD);
      c.using_lkg = 1;
      CHECK(rsh_boot_tick(&c, 1, 3000, 180000) == RSH_BOOT_SUCCESS);
      CHECK(c.consecutive_failures == 0);
    }
  }

  // 11) Safe mode + boot counter (FW-3, safemode.h; thresholds §4.4).
  {
    rsh_boot_counter_t c;
    rsh_boot_counter_init(&c);
    // 3 lagatar fail -> last-known-good; LKG bhi 3 fail -> factory.
    for (int i = 0; i < 3; i++) {
      rsh_boot_begin(&c);
      rsh_boot_record_outcome(&c, 0);
    }
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_LAST_KNOWN_GOOD);
    CHECK(rsh_boot_decide_source(&c, 0) == RSH_BOOT_SRC_FACTORY);
    c.using_lkg = 1;
    for (int i = 0; i < 3; i++) {
      rsh_boot_begin(&c);
      rsh_boot_record_outcome(&c, 0);
    }
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_FACTORY);
    // Kamyab boot dono counter zero karta hai.
    rsh_boot_begin(&c);
    rsh_boot_record_outcome(&c, 1);
    CHECK(rsh_boot_decide_source(&c, 1) == RSH_BOOT_SRC_CURRENT);
    // Ek boot ka natija dobara darj nahi hota (double-count nahi).
    // (using_lkg upar wale LKG block se 1 reh gaya tha — is block ke liye
    //  current-boot scenario chahiye, is liye wapas 0 karo.)
    c.using_lkg = 0;
    rsh_boot_begin(&c);
    rsh_boot_record_outcome(&c, 0);
    rsh_boot_record_outcome(&c, 0);
    CHECK(c.consecutive_failures == 1);
    // Boot tick: AP deadline (15 s) ke andar up -> SUCCESS; der se -> FAILED.
    rsh_boot_counter_t t;
    rsh_boot_counter_init(&t);
    rsh_boot_begin(&t);
    CHECK(rsh_boot_tick(&t, 0, 14000, 0) == RSH_BOOT_WAITING);
    CHECK(rsh_boot_tick(&t, 1, 14000, 0) == RSH_BOOT_SUCCESS);
    rsh_boot_begin(&t);
    CHECK(rsh_boot_tick(&t, 0, 15000, 0) == RSH_BOOT_FAILED);
    CHECK(rsh_boot_tick(&t, 0, 20000, 0) == RSH_BOOT_WAITING);  // darj ho chuka
    // BOOT-hold: <5 s kuch nahi; 5 s -> safe mode; 10 s -> key-reset-armed.
    rsh_boothold_t d;
    rsh_boothold_init(&d);
    CHECK(rsh_boothold_update(&d, 1, 0) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&d, 1, 4999) == RSH_HOLD_NONE);
    CHECK(rsh_boothold_update(&d, 1, 5000) == RSH_HOLD_SAFE_MODE);
    CHECK(rsh_boothold_update(&d, 1, 5001) == RSH_HOLD_NONE);  // ek dafa fire
    CHECK(rsh_boothold_update(&d, 1, 10000) == RSH_HOLD_KEY_RESET_ARMED);
    CHECK(rsh_boothold_update(&d, 0, 10001) == RSH_HOLD_NONE);  // release reset
    CHECK(rsh_boothold_update(&d, 1, 11000) == RSH_HOLD_NONE);  // naya hold
    CHECK(rsh_boothold_update(&d, 1, 16000) == RSH_HOLD_SAFE_MODE);
    // Safe mode: config blocked; armed window 120 s ke baad reboot signal.
    rsh_safe_mode_t s;
    rsh_safe_mode_init(&s);
    CHECK(rsh_safe_mode_blocks_config(&s) == 0);
    rsh_safe_mode_enter(&s, 0, 1000);
    CHECK(rsh_safe_mode_blocks_config(&s) == 1);
    CHECK(rsh_safe_mode_tick(&s, 500000) == 0);  // safe mode ki expiry nahi
    rsh_safe_mode_init(&s);
    rsh_safe_mode_enter(&s, 1, 1000);
    CHECK(rsh_safe_mode_tick(&s, 1000 + 119999) == 0);
    CHECK(rsh_safe_mode_tick(&s, 1000 + 120000) == 1);
  }

  // 12) Config transaction reconciliation (Phase E WS5): `config_txn.h`
  // (parallel design — contiguous-only chunks, blob CRC16, koi
  // live-apply/previous-copy nahi) HATA DIYA GAYA — waahid live engine
  // `config_transaction.h` hai (contract §3.5/§3.6). Purane §12 ke
  // scenarios yahan live API par port hain (baaqi coverage upar §11
  // wale live-engine sections mein hai).
  //
  // NOTE — do semantic farq (jaan-boojh kar, live engine jeetta hai):
  //   (a) Confirm boundary: purana engine theek 30 s par confirm OK
  //       karta tha (`> 30 s` rollback); live engine deadline par
  //       (`>= 30 s`) rollback karta hai — confirm 29999 ms par OK,
  //       30000 ms par UNSUPPORTED + rollback. Contract §3.6 "30 s mein
  //       confirm na ho -> rollback" dono parhne deta hai; live engine
  //       ka `>=` faisla D28 ke saath hai (deadline guzar gayi = khatam).
  //   (b) Stage jab COMMITTED ho: purana engine `false`, live engine
  //       BUSY (7) — confirm window mein conflicting op (§3.9).
  //   (c) Blob integrity: purana engine CRC16, live engine CRC32
  //       trailer (§3.6) — bad blob COMMIT par BAD_CRC deta hai.
  {
    uint8_t factory[256];
    size_t flen = rsh_cfg_factory_blob(factory);
    CHECK(flen == 188);  // Phase G C3: 11 records (pehle 7, 124 B)
    uint8_t newblob[256];
    size_t nlen = rsh_cfg_factory_blob(newblob);
    rsh_put_u16(newblob + 6, 2);
    rsh_put_u32(newblob + nlen - 4, rsh_crc32(newblob, nlen - 4));
    uint16_t sb = 0, tot = 0;
    rsh_cfg_txn_t t;
    rsh_cfg_txn_init(&t);
    CHECK(rsh_cfg_txn_load_current(&t, factory, flen) == RSH_CTRL_OK);
    // Happy path (live API): 2 chunks, commit, 29 s mein confirm -> live.
    CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, 62,
                              rsh_crc16_ccitt(newblob, 62), newblob, &sb, &tot)
          == RSH_CTRL_OK);
    CHECK(rsh_cfg_stage_chunk(&t, 62, (uint16_t)nlen, (uint16_t)(nlen - 62),
                              rsh_crc16_ccitt(newblob + 62, (uint16_t)(nlen - 62)),
                              newblob + 62, &sb, &tot) == RSH_CTRL_OK);
    CHECK(t.state == RSH_TXN_STAGED);
    CHECK(rsh_cfg_confirm(&t, 1000) == RSH_CTRL_UNSUPPORTED);  // commit se pehle nahi
    CHECK(rsh_cfg_commit(&t, 1000, NULL, NULL) == RSH_CTRL_OK);
    CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen, 16,
                              rsh_crc16_ccitt(newblob, 16), newblob, &sb, &tot)
          == RSH_CTRL_BUSY);  // (b) committed mein stage = BUSY
    CHECK(rsh_cfg_confirm(&t, 1000 + 29000) == RSH_CTRL_OK);
    CHECK(t.cfg_version == 2 && t.state == RSH_TXN_IDLE);
    CHECK(memcmp(t.current, newblob, nlen) == 0);
    // Boundary (a): theek 30 s par confirm NAHI hota — rollback ho chuka.
    // Doosri blob (version 3) commit karo: rollback pre-commit current
    // (version 2) wapas lata hai, factory nahi — previous copy hi asliyat hai.
    uint8_t blob3[256];
    size_t nlen3 = rsh_cfg_factory_blob(blob3);
    rsh_put_u16(blob3 + 6, 3);
    rsh_put_u32(blob3 + nlen3 - 4, rsh_crc32(blob3, nlen3 - 4));
    CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)nlen3, (uint16_t)nlen3,
                              rsh_crc16_ccitt(blob3, (uint16_t)nlen3),
                              blob3, &sb, &tot) == RSH_CTRL_OK);
    CHECK(rsh_cfg_commit(&t, 0, NULL, NULL) == RSH_CTRL_OK);
    CHECK(t.cfg_version == 3);  // live apply ho gayi (confirm se pehle)
    CHECK(rsh_cfg_confirm(&t, 30000) == RSH_CTRL_UNSUPPORTED);
    CHECK(t.state == RSH_TXN_IDLE && memcmp(t.current, newblob, nlen) == 0);
    CHECK(t.cfg_version == 2);  // rollback: version 2 wapas
    CHECK(rsh_cfg_tick(&t, 30001) == 0);  // dobara rollback nahi
    // Bad CRC (c): COMMIT fail, STAGED rehta hai, dobara commit bhi fail.
    uint8_t badblob[256];
    size_t blen = rsh_cfg_factory_blob(badblob);
    badblob[blen - 1] ^= 0xFF;  // CRC32 trailer kharab
    CHECK(rsh_cfg_stage_chunk(&t, 0, (uint16_t)blen, (uint16_t)blen,
                              rsh_crc16_ccitt(badblob, (uint16_t)blen),
                              badblob, &sb, &tot) == RSH_CTRL_OK);
    uint8_t errs[8];
    uint8_t nerr = 0;
    CHECK(rsh_cfg_commit(&t, 0, errs, &nerr) == RSH_CTRL_BAD_CRC);
    CHECK(t.state == RSH_TXN_STAGED);
    CHECK(rsh_cfg_commit(&t, 0, errs, &nerr) == RSH_CTRL_BAD_CRC);
    CHECK(memcmp(t.current, newblob, nlen) == 0);  // live config chhooi nahi
  }

  // 13) Network pure logic (FW-7, net_logic.h): channel pick, HELO timeout,
  // txFail counter (F3).
  {
    int empty_load[14] = {0};
    CHECK(rsh_pick_channel(empty_load) == 1);  // tie -> chhota channel
    int load1[14] = {0};
    load1[1] = 5;  // channel 1 bheer wala -> 6 (tie 6/11, chhota)
    CHECK(rsh_pick_channel(load1) == 6);
    int load2[14] = {0};
    load2[6] = 3; load2[11] = 1;  // 1 sab se khaali
    CHECK(rsh_pick_channel(load2) == 1);
    int load3[14] = {0};
    load3[2] = 4;  // ch 1 ka padosi (overlap) — 1 phir bhi jeetta hai
    CHECK(rsh_channel_score(load3, 1) == 8);   // 4*2 (d=1)
    CHECK(rsh_channel_score(load3, 6) == 0);
    // HELO timeout 2000 ms, wrap-safe.
    CHECK(rsh_pc_active(1000, 2999, 2000) == true);
    CHECK(rsh_pc_active(1000, 3000, 2000) == false);
    CHECK(rsh_pc_active(0xFFFFFF00u, 0x100, 2000) == true);   // wrap ke paar
    CHECK(rsh_pc_active(0xFFFFFF00u, 0x1000, 2000) == false);
    // TxStats: kamyab + fail alag gine jate hain.
    TxStats ts;
    CHECK(ts.record(true) == true && ts.sent == 1 && ts.tx_fail == 0);
    CHECK(ts.record(false) == false && ts.tx_fail == 1);
    CHECK(ts.record(false) == false && ts.tx_fail == 2 && ts.sent == 1);
  }

  // 14) WebOTA validation (FW-9, ota_validate.h): magic + anti-downgrade +
  // size cap + har endpoint par auth (F1/F9).
  {
    CHECK(rsh_version_compare({4, 0, 0}, {4, 0, 0}) == 0);
    CHECK(rsh_version_compare({4, 0, 1}, {4, 0, 0}) > 0);
    CHECK(rsh_version_compare({3, 9, 9}, {4, 0, 0}) < 0);
    CHECK(rsh_version_compare({4, 1, 0}, {4, 0, 9}) > 0);
    uint8_t good[4] = {0xE9, 0x02, 0x02, 0x00};
    uint8_t bad[4] = {0x00, 0x02, 0x02, 0x00};
    CHECK(ota_first_chunk_ok(good, 4, {4, 0, 0}, {4, 0, 1}) == true);
    CHECK(ota_first_chunk_ok(good, 4, {4, 0, 0}, {4, 0, 0}) == true);   // barabar = reinstall, allowed (SEC-C-05)
    CHECK(ota_first_chunk_ok(good, 4, {4, 0, 0}, {3, 9, 9}) == false);  // downgrade
    CHECK(ota_first_chunk_ok(bad, 4, {4, 0, 0}, {4, 0, 1}) == false);   // ghalat magic
    CHECK(ota_first_chunk_ok(NULL, 0, {4, 0, 0}, {4, 0, 1}) == false);
    CHECK(ota_first_chunk_ok(good, 0, {4, 0, 0}, {4, 0, 1}) == false);
    CHECK(ota_total_size_ok(0) == false);
    CHECK(ota_total_size_ok(0x140000) == true);    // app slot cap
    CHECK(ota_total_size_ok(0x140001) == false);
    CHECK(ota_endpoint_allowed(true) == true);
    CHECK(ota_endpoint_allowed(false) == false);  // F1: baghair auth kuch nahi
  }

  // 15) Heap + restart policy (FW-10, heap_policy.h, D17).
  {
    HeapPolicy p;
    heap_policy_reset(&p);
    CHECK(heap_policy_update(NULL, 100000, 20000, 0) == 0);
    CHECK(heap_policy_update(&p, 100000, 20000, 0) == 0);  // baseline
    for (int i = 0; i < 9; i++)
      CHECK(heap_policy_update(&p, 100000, 4000, (uint32_t)(i + 1) * 10000) == 0);
    CHECK(heap_policy_update(&p, 100000, 4000, 100000) == 1);  // 10th cycle
    CHECK(heap_policy_update(&p, 100000, 20000, 110000) == 1);  // latched
    // Ek saaf cycle streak torti hai — 5 kharab + 1 saaf + 9 kharab = no restart.
    HeapPolicy q;
    heap_policy_reset(&q);
    CHECK(heap_policy_update(&q, 100000, 20000, 0) == 0);
    for (int i = 0; i < 5; i++)
      CHECK(heap_policy_update(&q, 100000, 4000, (uint32_t)(i + 1) * 10000) == 0);
    CHECK(heap_policy_update(&q, 100000, 20000, 60000) == 0);  // saaf — reset
    for (int i = 0; i < 9; i++)
      CHECK(heap_policy_update(&q, 100000, 4000, (uint32_t)(70000 + i * 10000)) == 0);
    CHECK(heap_policy_update(&q, 100000, 4000, 160000) == 1);
    // 5-min window mein 20%+ girawat -> restart; 10% girawat -> nahi.
    HeapPolicy r;
    heap_policy_reset(&r);
    CHECK(heap_policy_update(&r, 100000, 50000, 0) == 0);
    CHECK(heap_policy_update(&r, 90000, 50000, 100000) == 0);   // -10%
    CHECK(heap_policy_update(&r, 79000, 50000, 200000) == 1);   // -21%
    // Window rollover: nayi baseline, purani girawat bhool jao.
    HeapPolicy w;
    heap_policy_reset(&w);
    CHECK(heap_policy_update(&w, 100000, 50000, 0) == 0);
    CHECK(heap_policy_update(&w, 90000, 50000, 300000) == 0);   // window khatam
    CHECK(heap_policy_update(&w, 85000, 50000, 310000) == 0);   // nayi baseline se -5%
  }

  // 16) Modules (FW-11, modules.h): GET_CAPS table/body, on/off, H-pattern.
  {
    RshCapsEntry table[15];
    CHECK(rsh_caps_default_table(NULL, 15) == 0);
    CHECK(rsh_caps_default_table(table, 14) == 0);  // chhota buffer
    CHECK(rsh_caps_default_table(table, 15) == 15);
    CHECK(table[0].pin == PIN_STEER && table[0].module == RSH_MOD_STEERING);
    CHECK(table[3].pin == PIN_HANDBRAKE_ADC && table[3].module == RSH_MOD_HANDBRAKE);
    CHECK(table[7].pin == PIN_HORN_BTN && table[7].module == RSH_MOD_HORN);
    CHECK(table[14].pin == PIN_LED_STRIP && table[14].kind == RSH_KIND_LED_STRIP);
    // caps bits 5-7 hamesha 0 (§3.4).
    RshCapsEntry e = rsh_caps_entry(4, RSH_KIND_DIGITAL_IN, RSH_MOD_HORN, 0xFF);
    CHECK(e.caps == 0x1F);
    CHECK(e.reserved[0] == 0 && e.reserved[3] == 0);
    // GET_CAPS body: 18 + 8*15 = 138 B.
    uint8_t mac[6] = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF};
    uint8_t body[256];
    CHECK(rsh_caps_build_body(NULL, sizeof(body), mac, 6, 0, table, 15) == 0);
    CHECK(rsh_caps_build_body(body, 100, mac, 6, 0, table, 15) == 0);
    CHECK(rsh_caps_build_body(body, sizeof(body), mac, 6, 0, table, 41) == 0);
    CHECK(rsh_caps_build_body(body, sizeof(body), mac, 6, 0, table, 15) == 138);
    CHECK(body[0] == RSH_CTRL_OK && body[7] == 4 && body[8] == 0 && body[9] == 1);  // fw 4.0.1 (Round 2 W3 FIX-1)
    CHECK(body[10] == 2 && body[11] == 1);  // proto 2.1
    CHECK(body[12] == 8 && body[13] == 64 && body[14] == 4);
    CHECK(body[15] == 6 && body[17] == 15);
    CHECK(body[18 + 3 * 8] == PIN_HANDBRAKE_ADC);
    // On/off (F7): band module ka axis/button bit clear hota hai.
    // Phase F WS5: handbrake slot 4 hai (mask 0x17), clutch slot 3 off hai.
    RshModuleEnable en = {1, 1, 1, 1, 1, 0, 0};
    CHECK(rsh_stream_axis_mask(&en) == 0x17);
    CHECK(rsh_stream_button_bits(&en, 1, 1, 1, 1, 1, 0) == 0x1F);
    en.handbrake = 0;
    CHECK(rsh_stream_axis_mask(&en) == 0x07);  // handbrake axis bit gaya
    en.horn = 0;
    CHECK(rsh_stream_button_bits(&en, 1, 1, 1, 1, 1, 0) == 0x0F);  // horn bit gaya
    en.shifter = 0;
    CHECK(rsh_stream_button_bits(&en, 1, 1, 1, 1, 1, 0) == 0x0C);
    CHECK(rsh_stream_axis_mask(NULL) == 0x07);  // NULL = core modules hi
    // H-pattern decode: ek bit -> gear, 0 ya 2+ bits -> neutral (0).
    CHECK(rsh_shifter_hpattern_gear(0x01) == 1);
    CHECK(rsh_shifter_hpattern_gear(0x20) == 6);
    CHECK(rsh_shifter_hpattern_gear(0x00) == 0);
    CHECK(rsh_shifter_hpattern_gear(0x03) == 0);
    CHECK(rsh_shifter_hpattern_gear(0x3F) == 0);
    CHECK(rsh_shifter_hpattern_gear(0xFF) == 0);  // sirf low 6 bits
    // H-pattern + encoders conflict (shared pins 16/17/19).
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_HPATTERN, 1) == 1);
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_HPATTERN, 0) == 0);
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_SEQUENTIAL, 1) == 0);
    // Global enable set/get (wapas default par restore karo).
    RshModuleEnable off = {0, 0, 0, 0, 0, 0, 0};
    rsh_modules_set_enabled(&off);
    CHECK(rsh_modules_handbrake_enabled() == 0 && rsh_modules_led_enabled() == 0);
    RshModuleEnable on = {1, 1, 1, 1, 1, 0, 0};
    rsh_modules_set_enabled(&on);
    CHECK(rsh_modules_handbrake_enabled() == 1 && rsh_modules_led_enabled() == 1);
    CHECK(rsh_modules_clutch_enabled() == 0 && rsh_modules_shifter_mode() == 0);
  }

  // 17) Encoders + LED patterns (FW-12, encoders.h / led_patterns.h).
  {
    // Quadrature: ek poora cycle (00->10->11->01->00) = +4; ulta = -4.
    RshEncoder enc;
    rsh_encoder_init(&enc);
    rsh_encoder_step(&enc, 0, 0);
    rsh_encoder_step(&enc, 1, 0);
    rsh_encoder_step(&enc, 1, 1);
    rsh_encoder_step(&enc, 0, 1);
    rsh_encoder_step(&enc, 0, 0);
    CHECK(enc.count == 4);
    rsh_encoder_step(&enc, 0, 1);
    rsh_encoder_step(&enc, 1, 1);
    rsh_encoder_step(&enc, 1, 0);
    rsh_encoder_step(&enc, 0, 0);
    CHECK(enc.count == 0);
    // Glitch (dono bits ek saath) count nahi badalta — drift nahi.
    rsh_encoder_step(&enc, 1, 1);  // 00 -> 11 ek saath
    CHECK(enc.count == 0);
    rsh_encoder_step(&enc, 0, 0);  // 11 -> 00 ek saath
    CHECK(enc.count == 0);
    // 100 turns aage + 100 peeche = exact 0 (drift test).
    const uint8_t cyc[4][2] = {{0, 0}, {1, 0}, {1, 1}, {0, 1}};
    RshEncoder big;
    rsh_encoder_init(&big);
    rsh_encoder_step(&big, 0, 0);
    for (int turn = 0; turn < 100; turn++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&big, cyc[s % 4][0], cyc[s % 4][1]);
    CHECK(big.count == 400);
    for (int turn = 0; turn < 100; turn++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&big, cyc[(4 - s) % 4][0], cyc[(4 - s) % 4][1]);
    CHECK(big.count == 0);
    // i16 wrap-aware delta (C3): 32760 -> -32756 = +20, ulta nahi.
    CHECK(rsh_encoder_delta(-32756, 32760) == 20);
    CHECK(rsh_encoder_delta(5, -5) == 10);
    CHECK(rsh_encoder_delta(-32768, 32767) == 1);
    CHECK(rsh_encoder_delta(32767, -32768) == -1);
    // LED validate (§3.7 + R-L9): pin 21 direct rig mein, expanders mein nahi.
    RshLedPattern cfg = {RSH_LED_BLINK, 255, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 1) == RSH_CTRL_OK);
    CHECK(rsh_led_pattern_validate(&cfg, 21, 1, 1) == RSH_CTRL_PIN_CONFLICT);
    CHECK(rsh_led_pattern_validate(&cfg, 4, 0, 1) == RSH_CTRL_PIN_CONFLICT);
    CHECK(rsh_led_pattern_validate(&cfg, 4, 1, 1) == RSH_CTRL_OK);  // expander: freed pin
    CHECK(rsh_led_pattern_validate(&cfg, 2, 1, 1) == RSH_CTRL_PIN_CONFLICT);  // status LED
    CHECK(rsh_led_pattern_validate(&cfg, 21, 0, 0) == RSH_CTRL_UNSUPPORTED);  // no module
    RshLedPattern bad_id = {9, 0, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_pattern_validate(&bad_id, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
    RshLedPattern bad_rsv = {RSH_LED_SOLID, 0, 0, 0, 1000, 0, 1};
    CHECK(rsh_led_pattern_validate(&bad_rsv, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
    RshLedPattern bad_per = {RSH_LED_BLINK, 0, 0, 0, 50, 0, 0};
    CHECK(rsh_led_pattern_validate(&bad_per, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
    RshLedPattern ok_per = {RSH_LED_BLINK, 0, 0, 0, 100, 0, 0};
    CHECK(rsh_led_pattern_validate(&ok_per, 21, 0, 1) == RSH_CTRL_OK);
    CHECK(rsh_led_pattern_validate(NULL, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
    // Hue wheel: 0 = laal, 85 = sabz ki taraf.
    uint8_t r, g, b;
    rsh_led_hue_rgb(0, &r, &g, &b);
    CHECK(r == 255 && g == 0 && b == 0);
    rsh_led_hue_rgb(85, &r, &g, &b);
    CHECK(g == 255 && r < 10 && b == 0);
    // SOLID 5 s (repeats 0): t=4999 on, t=5000 off + done.
    RshLedPatternState st;
    RshLedPattern solid = {RSH_LED_SOLID, 10, 20, 30, 1000, 0, 0};
    rsh_led_pattern_start(&st, &solid, 1000);
    rsh_led_pattern_pixel(&st, 1000, 0, 4, &r, &g, &b);
    CHECK(r == 10 && g == 20 && b == 30);
    CHECK(rsh_led_pattern_done(&st, 5999) == 0);
    rsh_led_pattern_pixel(&st, 6000, 0, 4, &r, &g, &b);
    CHECK(r == 0 && g == 0 && b == 0);
    CHECK(rsh_led_pattern_done(&st, 6000) == 1);
    // BLINK period 1000: 0-499 on, 500-999 off, 1000 on.
    RshLedPattern blink = {RSH_LED_BLINK, 255, 255, 255, 1000, 0, 0};
    rsh_led_pattern_start(&st, &blink, 0);
    rsh_led_pattern_pixel(&st, 250, 0, 4, &r, &g, &b);
    CHECK(r == 255);
    rsh_led_pattern_pixel(&st, 750, 0, 4, &r, &g, &b);
    CHECK(r == 0);
    rsh_led_pattern_pixel(&st, 1250, 0, 4, &r, &g, &b);
    CHECK(r == 255);
    // CHASE period 400, 4 LEDs: har 100 ms agla pixel.
    RshLedPattern chase = {RSH_LED_CHASE, 9, 9, 9, 400, 0, 0};
    rsh_led_pattern_start(&st, &chase, 0);
    rsh_led_pattern_pixel(&st, 0, 0, 4, &r, &g, &b);
    CHECK(r == 9);
    rsh_led_pattern_pixel(&st, 0, 1, 4, &r, &g, &b);
    CHECK(r == 0);
    rsh_led_pattern_pixel(&st, 150, 1, 4, &r, &g, &b);
    CHECK(r == 9);
    rsh_led_pattern_pixel(&st, 350, 3, 4, &r, &g, &b);
    CHECK(r == 9);
    // RAINBOW: waqt ke saath rang badalta hai.
    RshLedPattern rb = {RSH_LED_RAINBOW, 0, 0, 0, 1000, 0, 0};
    rsh_led_pattern_start(&st, &rb, 0);
    uint8_t r0, g0, b0;
    rsh_led_pattern_pixel(&st, 0, 0, 1, &r0, &g0, &b0);
    rsh_led_pattern_pixel(&st, 250, 0, 1, &r, &g, &b);
    CHECK(!(r == r0 && g == g0 && b == b0));
    // OFF: foran done, pixel hamesha (0,0,0).
    RshLedPattern off = {RSH_LED_OFF, 0, 0, 0, 1000, 0, 0};
    rsh_led_pattern_start(&st, &off, 0);
    CHECK(rsh_led_pattern_done(&st, 0) == 1);
    rsh_led_pattern_pixel(&st, 0, 0, 4, &r, &g, &b);
    CHECK(r == 0 && g == 0 && b == 0);
  }

  // 18) Control extra negatives (FW-8, T2): structure / build / session ring.
  {
    rsh_v2_ctrl_t cf;
    uint8_t test_key[32], test_nonce[16];
    for (int i = 0; i < 32; i++) test_key[i] = (uint8_t)i;
    for (int i = 0; i < 16; i++) test_nonce[i] = (uint8_t)(0xA0 + i);
    uint8_t frame[64] = {0};
    // Header se chhota frame -> TOO_SHORT (sirf <13 B par).
    CHECK(rsh_v2_ctrl_parse_structure(frame, 0, &cf) == RSH_V2_TOO_SHORT);
    CHECK(rsh_v2_ctrl_parse_structure(frame, 12, &cf) == RSH_V2_TOO_SHORT);
    CHECK(rsh_v2_ctrl_parse_structure(NULL, 29, &cf) == RSH_V2_TOO_SHORT);
    // Verify: null key/nonce/hmac -> BAD_TAG (koi crash nahi).
    const uint8_t caps[29] = {0x52, 0x53, 0x48, 0x43, 0x01, 0x01, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0xee, 0x8d, 0x10, 0x07, 0x93, 0x58,
      0xd9, 0x93, 0x6f, 0xe7, 0x43, 0x68, 0xf8, 0xb6, 0x61};
    CHECK(rsh_v2_ctrl_verify(caps, sizeof(caps), NULL, test_nonce, test_hmac16, &cf) == RSH_V2_BAD_TAG);
    CHECK(rsh_v2_ctrl_verify(caps, sizeof(caps), test_key, NULL, test_hmac16, &cf) == RSH_V2_BAD_TAG);
    CHECK(rsh_v2_ctrl_verify(caps, sizeof(caps), test_key, test_nonce, NULL, &cf) == RSH_V2_BAD_TAG);
    // Build: null buf/hmac, chhota buf -> 0.
    uint8_t out[64];
    CHECK(rsh_v2_ctrl_build(NULL, sizeof(out), 0x01, 1, 0, NULL, 0,
                            test_key, test_nonce, test_hmac16) == 0);
    CHECK(rsh_v2_ctrl_build(out, sizeof(out), 0x01, 1, 0, NULL, 0,
                            test_key, test_nonce, NULL) == 0);
    CHECK(rsh_v2_ctrl_build(out, 10, 0x01, 1, 0, NULL, 0,
                            test_key, test_nonce, test_hmac16) == 0);
    // Session ring (32 entries): 33 distinct accept ke baad sab se purana
    // pair evict hota hai — us ka dobara aana DUPLICATE nahi, REPLAY hai.
    rsh_ctrl_session_t sess;
    rsh_ctrl_session_init(&sess);
    for (int i = 0; i < 33; i++)
      CHECK(rsh_ctrl_session_classify(&sess, (uint16_t)i, (uint32_t)i) == RSH_DEC_ACCEPT);
    CHECK(rsh_ctrl_session_classify(&sess, 32, 32) == RSH_DEC_DUPLICATE);
    CHECK(rsh_ctrl_session_classify(&sess, 0, 0) == RSH_DEC_REPLAY);
    CHECK(rsh_ctrl_session_classify(&sess, 33, 33) == RSH_DEC_ACCEPT);
  }

  // 19) FW-7 extensions + FW-8 dispatcher (Workstream 3, SEC-D-01/06/07/08/09).
  {
    uint8_t test_key[32], test_nonce[16], wrong_key[32];
    for (int i = 0; i < 32; i++) { test_key[i] = (uint8_t)i; wrong_key[i] = 0xFF; }
    for (int i = 0; i < 16; i++) test_nonce[i] = (uint8_t)(0xA0 + i);

    // --- FW-7: RSSI-weighted scan load (F8) ---
    {
      // ch1: 2 strong (-40, w4 = 8), ch6: 3 weak (-85, w1 = 3),
      // ch11: 2 medium (-65, w3 = 6). Count-only (2,3,2) -> scores
      // (8,12,8) -> pick 1; RSSI-weighted (8,3,6) -> scores (32,12,24)
      // -> pick 6. RSSI farq karta hai (sirf ginna ghalat tha).
      rsh_scan_entry_t e[] = {
        {1, -40, 0}, {1, -40, 0},
        {6, -85, 0}, {6, -85, 0}, {6, -85, 0},
        {11, -65, 0}, {11, -65, 0},
      };
      int load[14];
      rsh_scan_load(e, 7, load);
      CHECK(load[1] == 8 && load[6] == 3 && load[11] == 6);
      CHECK(rsh_pick_channel(load) == 6);
      int count_load[14] = {0};
      count_load[1] = 2; count_load[6] = 3; count_load[11] = 2;
      CHECK(rsh_pick_channel(count_load) == 1);  // count-only -> 1 (tie)
      // Hidden strong net ch6 par — gin'a gaya to pick 11 hota (score 28
      // vs 24); exclude ho to pick 6 rehta hai.
      rsh_scan_entry_t e2[] = {
        {1, -40, 0}, {1, -40, 0},
        {6, -85, 0}, {6, -85, 0}, {6, -85, 0},
        {11, -65, 0}, {11, -65, 0},
        {6, -30, 1},  // hidden — ignore
      };
      rsh_scan_load(e2, 8, load);
      CHECK(load[6] == 3);
      CHECK(rsh_pick_channel(load) == 6);
      // Invalid channel ignore; khaali scan -> 1 (purana fallback).
      rsh_scan_entry_t e3[] = {{0, -40, 0}, {14, -40, 0}, {6, -40, 0}};
      rsh_scan_load(e3, 3, load);
      CHECK(load[6] == 4 && load[0] == 0);
      CHECK(rsh_pick_channel(load) == 1);
      rsh_scan_load(NULL, 0, load);
      CHECK(rsh_pick_channel(load) == 1);
      // Rescan policy (F8): 60 s se pehle kabhi nahi; achhi link par nahi.
      CHECK(rsh_rescan_needed(-60, 0, 60000) == 0);    // achhi link
      CHECK(rsh_rescan_needed(-85, 0, 10000) == 0);    // kharab lekin jald
      CHECK(rsh_rescan_needed(-85, 0, 60000) == 1);    // kharab + waqfa
      CHECK(rsh_rescan_needed(-128, 25, 60000) == 1);  // RSSI unknown + txFail
      CHECK(rsh_rescan_needed(-128, 0, 60000) == 0);   // RSSI unknown, txFail 0
      CHECK(rsh_rescan_needed(-70, 20, 60000) == 1);   // txFail delta hi kaafi
      CHECK(rsh_rescan_needed(-70, 19, 60000) == 0);   // 19 < 20
      // Channel switch: sirf sakhti se behtar + 5 min guzar gaye hon.
      CHECK(rsh_channel_switch_allowed(10, 20, 300000) == 1);
      CHECK(rsh_channel_switch_allowed(10, 20, 299999) == 0);
      CHECK(rsh_channel_switch_allowed(20, 20, 300000) == 0);  // barabar nahi
      CHECK(rsh_channel_switch_allowed(25, 20, 300000) == 0);  // battar nahi
      // HELO accept (SEC-D-08).
      CHECK(rsh_helo_accept(0, 0, 0, 0) == 1);        // pehla peer
      CHECK(rsh_helo_accept(1, 1, 1, 0) == 1);        // wahi IP, live
      CHECK(rsh_helo_accept(1, 1, 0, 0) == 0);        // doosra IP, live -> mana
      CHECK(rsh_helo_accept(1, 0, 0, 6000) == 1);     // idle + 6 s -> qabool
      CHECK(rsh_helo_accept(1, 0, 0, 4000) == 0);     // idle lekin 4 s -> flap
      CHECK(rsh_helo_accept(1, 0, 1, 0) == 1);        // wahi IP hamesha
      // HELLO reply throttle: 500 ms.
      CHECK(rsh_hello_reply_allowed(0) == 0);
      CHECK(rsh_hello_reply_allowed(499) == 0);
      CHECK(rsh_hello_reply_allowed(500) == 1);
      // Ping limiter (§9.4): burst 5, phir drop; refill 10/s; alag IP
      // alag bucket; 64 sources ke baad LRU evict.
      rsh_ping_limiter_t lim;
      rsh_ping_limiter_init(&lim);
      uint32_t ip1 = 0xC0A80402u;  // 192.168.4.2
      for (int i = 0; i < 5; i++) CHECK(rsh_ping_allow(&lim, ip1, 1000) == 1);
      CHECK(rsh_ping_allow(&lim, ip1, 1000) == 0);   // burst khatam
      CHECK(rsh_ping_allow(&lim, ip1, 1000) == 0);
      CHECK(rsh_ping_allow(&lim, ip1, 1100) == 1);   // +100 ms = 1 token
      CHECK(rsh_ping_allow(&lim, ip1, 1100) == 0);
      CHECK(rsh_ping_allow(&lim, 0xC0A80403u, 1100) == 1);  // doosra IP
      // 5 s idle -> full refill.
      for (int i = 0; i < 5; i++) CHECK(rsh_ping_allow(&lim, ip1, 10000) == 1);
      CHECK(rsh_ping_allow(&lim, ip1, 10000) == 0);
      rsh_ping_limiter_t lim2;
      rsh_ping_limiter_init(&lim2);
      for (uint32_t k = 0; k < 64; k++)
        for (int i = 0; i < 5; i++) CHECK(rsh_ping_allow(&lim2, 0x0A000000u + k, 2000) == 1);
      CHECK(rsh_ping_allow(&lim2, 0x0A000000u, 2000) == 0);  // sab khaali
      CHECK(rsh_ping_allow(&lim2, 0x0B000000u, 2000) == 1);  // 65th -> evict oldest
      CHECK(rsh_ping_allow(&lim2, 0x0A000000u, 2000) == 1);  // evicted -> fresh bucket
    }

    // --- FW-8: HELLO build golden (92 B, §2.2) — Python vector se match ---
    {
      uint8_t mac[6] = {0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF};
      const uint8_t hello_exp[92] = {0x52, 0x53, 0x48, 0x52, 0x02, 0x01, 0x09, 0x01,
        0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x04, 0x00, 0x00, 0x00, 0xa0, 0xa1,
        0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad,
        0xae, 0xaf, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15,
        0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x41, 0x62,
        0x33, 0x64, 0x45, 0x66, 0x36, 0x68, 0x4a, 0x6b, 0x39, 0x6d, 0x4e, 0x70,
        0x32, 0x72, 0x53, 0x74, 0x35, 0x76, 0x57, 0x78, 0x38, 0x59, 0x5a, 0x42};
      uint8_t out[92];
      size_t n = rsh_v2_build_hello(out, 0x09, 1, mac, 4, 0, 0, test_nonce, test_key,
                                    (const uint8_t *)"Ab3dEf6hJk9mNp2rSt5vWx8Y");
      CHECK(n == 92 && memcmp(out, hello_exp, 92) == 0);
      // Paired: key/password zero → parse OK, fields zero.
      uint8_t zeros32[32] = {0}, zeros24[24] = {0};
      n = rsh_v2_build_hello(out, 0x00, 0, mac, 4, 0, 0, test_nonce, zeros32, zeros24);
      CHECK(n == 92);
      rsh_v2_hello_t hp;
      CHECK(rsh_v2_parse_hello(out, 92, &hp) == RSH_V2_OK);
      CHECK(hp.flags == 0 && hp.session_echo == 0);
      uint8_t z56[56] = {0};
      CHECK(memcmp(hp.device_key, z56, 32) == 0);
      CHECK(memcmp(hp.wifi_password, z56, 24) == 0);
      CHECK(rsh_v2_build_hello(NULL, 0, 0, mac, 4, 0, 0, test_nonce, test_key, zeros24) == 0);
    }

    // --- FW-8: dispatcher (ctrl.h) — ghalat frame kabhi execute nahi ---
    {
      test_ctrl_ctx_t ctx = {0, 0, 0};
      rsh_ctrl_session_t sess;
      rsh_ctrl_session_init(&sess);
      rsh_ctrl_cache_t cache;
      rsh_ctrl_cache_init(&cache);
      uint8_t reply[1200];
      size_t rlen = 0;
      auto disp = [&](const uint8_t *fr, size_t fl) {
        rlen = 0;
        return rsh_ctrl_dispatch(fr, fl, test_key, test_nonce, test_hmac16,
                                 &sess, &cache, test_ctrl_handler, &ctx,
                                 reply, sizeof(reply), &rlen);
      };
      auto disp_key = [&](const uint8_t *fr, size_t fl, const uint8_t *k) {
        rlen = 0;
        return rsh_ctrl_dispatch(fr, fl, k, test_nonce, test_hmac16,
                                 &sess, &cache, test_ctrl_handler, &ctx,
                                 reply, sizeof(reply), &rlen);
      };
      rsh_v2_ctrl_t rf;
      const uint8_t ctrl_reply_exp[47] = {0x52, 0x53, 0x48, 0x43, 0x81, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x12, 0x00, 0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x04, 0x00, 0x00, 0x02, 0x01, 0x08, 0x40, 0x04, 0x06, 0x00, 0x00, 0x04,
        0xb2, 0x21, 0x24, 0x07, 0x63, 0x4f, 0xfd, 0x85, 0x52, 0xf4, 0x91, 0xba,
        0x7d, 0xcb, 0xc0};

      // Sahi GET_CAPS frame -> OK reply, golden bytes se byte-identical.
      uint8_t frame[64];
      size_t flen = rsh_v2_ctrl_build(frame, sizeof(frame), 0x01, 1, 0, NULL, 0,
                                      test_key, test_nonce, test_hmac16);
      CHECK(flen == 29);
      CHECK(disp(frame, flen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == 47 && memcmp(reply, ctrl_reply_exp, 47) == 0);
      CHECK(ctx.exec_caps == 1);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.cmd == 0x81 && rf.payload[0] == RSH_CTRL_OK);

      // Duplicate exact retransmit -> wahi cached reply, dobara execute nahi.
      uint8_t saved[1200];
      size_t saved_len = rlen;
      memcpy(saved, reply, rlen);
      CHECK(disp(frame, flen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == saved_len && memcmp(reply, saved, rlen) == 0);
      CHECK(ctx.exec_caps == 1);

      // Replay: naya reqId, wahi counter 0 -> REPLAY reply, no execute.
      uint8_t rep[64];
      size_t rplen = rsh_v2_ctrl_build(rep, sizeof(rep), 0x01, 99, 0, NULL, 0,
                                       test_key, test_nonce, test_hmac16);
      CHECK(disp(rep, rplen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == 30);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_REPLAY);
      CHECK(ctx.exec_caps == 1);

      // Retry: wahi reqId, NAYA counter -> ACCEPT (nayi execution, idempotent).
      uint8_t retry[64];
      size_t rtlen = rsh_v2_ctrl_build(retry, sizeof(retry), 0x01, 1, 1, NULL, 0,
                                       test_key, test_nonce, test_hmac16);
      CHECK(disp(retry, rtlen) == RSH_CTRL_DISP_REPLY);
      CHECK(ctx.exec_caps == 2);

      // Bad tag -> BAD_TAG reply, no execute.
      uint8_t bad[64];
      memcpy(bad, frame, flen);
      bad[flen - 1] ^= 0xFF;
      CHECK(disp(bad, flen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == 30);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_BAD_TAG);
      CHECK(ctx.exec_caps == 2);

      // Wrong key -> BAD_TAG reply (reply tag ghalat key se verify hota hai).
      CHECK(disp_key(frame, flen, wrong_key) == RSH_CTRL_DISP_REPLY);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, wrong_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_BAD_TAG);
      CHECK(ctx.exec_caps == 2);

      // Truncated / bad magic / len mismatch / too short -> silent DROP.
      CHECK(disp(frame, flen - 8) == RSH_CTRL_DISP_DROP && rlen == 0);
      memcpy(bad, frame, flen);
      bad[0] = 'X';
      CHECK(disp(bad, flen) == RSH_CTRL_DISP_DROP);
      memcpy(bad, frame, flen);
      bad[11] = 5;
      CHECK(disp(bad, flen) == RSH_CTRL_DISP_DROP);
      CHECK(disp(frame, 12) == RSH_CTRL_DISP_DROP);
      CHECK(ctx.exec_caps == 2);

      // Unknown cmd (0x7F, valid tag) -> UNSUPPORTED reply, execute nahi.
      uint8_t unk[64];
      size_t ulen = rsh_v2_ctrl_build(unk, sizeof(unk), 0x7F, 5, 2, NULL, 0,
                                      test_key, test_nonce, test_hmac16);
      CHECK(disp(unk, ulen) == RSH_CTRL_DISP_REPLY);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
      CHECK(ctx.exec_caps == 2 && ctx.exec_big == 0);

      // REBOOT (0x09, valid tag) -> OK + reboot flag (SEC-D-01 rasta).
      uint8_t rb[64];
      size_t rblen = rsh_v2_ctrl_build(rb, sizeof(rb), 0x09, 6, 3, NULL, 0,
                                       test_key, test_nonce, test_hmac16);
      CHECK(disp(rb, rblen) == RSH_CTRL_DISP_REPLY);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_OK && ctx.reboot == 1);

      // SEC-D-07a: reqId-tamper — valid frame ka reqId badlo (tag valid
      // rehta hai, reqId HMAC mein nahi) -> REPLAY, kabhi execute nahi.
      uint8_t tam[64];
      size_t tlen = rsh_v2_ctrl_build(tam, sizeof(tam), 0x01, 7, 4, NULL, 0,
                                      test_key, test_nonce, test_hmac16);
      CHECK(disp(tam, tlen) == RSH_CTRL_DISP_REPLY);
      CHECK(ctx.exec_caps == 3);
      uint8_t tam2[64];
      memcpy(tam2, tam, tlen);
      tam2[5] = 99; tam2[6] = 0;  // reqId 7 -> 99 (counter wahi 4)
      CHECK(disp(tam2, tlen) == RSH_CTRL_DISP_REPLY);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_REPLAY);
      CHECK(ctx.exec_caps == 3);

      // Bara reply (> 128 B) cache nahi hota — duplicate dobara execute.
      uint8_t big[1200];
      size_t blen = rsh_v2_ctrl_build(big, sizeof(big), 0x02, 8, 5, NULL, 0,
                                      test_key, test_nonce, test_hmac16);
      CHECK(disp(big, blen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == 13 + 201 + 16 && ctx.exec_big == 1);
      uint8_t saved_big[1200];
      size_t saved_big_len = rlen;
      memcpy(saved_big, reply, rlen);
      CHECK(disp(big, blen) == RSH_CTRL_DISP_REPLY);
      CHECK(rlen == saved_big_len && memcmp(reply, saved_big, rlen) == 0);
      CHECK(ctx.exec_big == 2);  // uncached duplicate -> idempotent re-execute

      // Counter wrap (SEC-D-07c, RFC 1982): fresh session.
      rsh_ctrl_session_t w;
      rsh_ctrl_session_init(&w);
      rsh_ctrl_cache_t c2;
      rsh_ctrl_cache_init(&c2);
      auto disp2 = [&](const uint8_t *fr, size_t fl) {
        rlen = 0;
        return rsh_ctrl_dispatch(fr, fl, test_key, test_nonce, test_hmac16,
                                 &w, &c2, test_ctrl_handler, &ctx,
                                 reply, sizeof(reply), &rlen);
      };
      uint8_t wf[64];
      size_t wflen = rsh_v2_ctrl_build(wf, sizeof(wf), 0x01, 10, 0xFFFFFFFEu, NULL, 0,
                                       test_key, test_nonce, test_hmac16);
      CHECK(disp2(wf, wflen) == RSH_CTRL_DISP_REPLY && ctx.exec_caps == 4);
      wflen = rsh_v2_ctrl_build(wf, sizeof(wf), 0x01, 11, 0xFFFFFFFFu, NULL, 0,
                                test_key, test_nonce, test_hmac16);
      CHECK(disp2(wf, wflen) == RSH_CTRL_DISP_REPLY && ctx.exec_caps == 5);
      wflen = rsh_v2_ctrl_build(wf, sizeof(wf), 0x01, 12, 0, NULL, 0,
                                test_key, test_nonce, test_hmac16);
      CHECK(disp2(wf, wflen) == RSH_CTRL_DISP_REPLY && ctx.exec_caps == 6);  // wrap accept
      wflen = rsh_v2_ctrl_build(wf, sizeof(wf), 0x01, 13, 0xFFFFFFFFu, NULL, 0,
                                test_key, test_nonce, test_hmac16);
      CHECK(disp2(wf, wflen) == RSH_CTRL_DISP_REPLY && ctx.exec_caps == 6);
      CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf) == RSH_V2_OK);
      CHECK(rf.payload[0] == RSH_CTRL_REPLAY);  // purana counter wrap ke baad bhi
      wflen = rsh_v2_ctrl_build(wf, sizeof(wf), 0x01, 11, 0xFFFFFFFFu, NULL, 0,
                                test_key, test_nonce, test_hmac16);
      CHECK(disp2(wf, wflen) == RSH_CTRL_DISP_REPLY && ctx.exec_caps == 6);  // duplicate
    }
  }

  // 20) WebOTA signed OTA (FW-9, SEC-C-06 / SEC-D-04, ota_validate.h):
  // version parse + marker scanner + trailer + streaming digest + Ed25519.
  {
    // Version parse (strict a.b.c).
    RshVersion v{};
    CHECK(rsh_parse_version("4.0.0", 5, &v) && v.major == 4 && v.minor == 0 && v.patch == 0);
    CHECK(rsh_parse_version("10.20.30", 8, &v) && v.major == 10 && v.minor == 20 && v.patch == 30);
    CHECK(!rsh_parse_version("4.0", 3, &v));
    CHECK(!rsh_parse_version("4.0.0.0", 7, &v));
    CHECK(!rsh_parse_version("256.0.0", 7, &v));
    CHECK(!rsh_parse_version("a.b.c", 5, &v));
    CHECK(!rsh_parse_version("", 0, &v));
    CHECK(!rsh_parse_version("4.0.0", 5, NULL));

    // Marker scanner: ek chunk mein (firmware jaisa: marker + NUL + version).
    RshVersionScan sc;
    rsh_version_scan_init(&sc);
    const uint8_t img1[] = "xxVersion <b>" "\x00" "4.0.0</b>yy";
    rsh_version_scan_feed(&sc, img1, sizeof(img1) - 1);
    CHECK(sc.found && sc.version.major == 4 && sc.version.minor == 0 && sc.version.patch == 0);
    // Marker chunk boundary par split ho.
    RshVersionScan sc2;
    rsh_version_scan_init(&sc2);
    const uint8_t p1[] = "zzVersion <";
    const uint8_t p2[] = "b>" "\x00" "4.1.2</b>";
    rsh_version_scan_feed(&sc2, p1, sizeof(p1) - 1);
    CHECK(!sc2.found);
    rsh_version_scan_feed(&sc2, p2, sizeof(p2) - 1);
    CHECK(sc2.found && sc2.version.major == 4 && sc2.version.minor == 1 && sc2.version.patch == 2);
    // Version run chunk ke aakhir mein adhura ho — agle chunk ka intezar.
    RshVersionScan sc6;
    rsh_version_scan_init(&sc6);
    const uint8_t q1[] = "Version <b>4.0";
    rsh_version_scan_feed(&sc6, q1, sizeof(q1) - 1);
    CHECK(!sc6.found);
    const uint8_t q2[] = ".0</b>";
    rsh_version_scan_feed(&sc6, q2, sizeof(q2) - 1);
    CHECK(sc6.found && sc6.version.major == 4 && sc6.version.patch == 0);
    // Marker nahi — kabhi found nahi.
    RshVersionScan sc3;
    rsh_version_scan_init(&sc3);
    const uint8_t p3[] = "no version here at all, just bytes";
    rsh_version_scan_feed(&sc3, p3, sizeof(p3) - 1);
    CHECK(!sc3.found);
    // Marker + na-jaiz version (sirf 2 hisse) — found nahi hona chahiye.
    RshVersionScan sc4;
    rsh_version_scan_init(&sc4);
    const uint8_t p4[] = "Version <b>9.9</b>";
    rsh_version_scan_feed(&sc4, p4, sizeof(p4) - 1);
    CHECK(!sc4.found);
    // 9.9.9 marker mil sakta hai (attacker bhi likh sakta hai — SEC-D-04),
    // lekin asliyat signature se sabit hoti hai, marker se nahi.
    RshVersionScan sc5;
    rsh_version_scan_init(&sc5);
    const uint8_t p5[] = "Version <b>9.9.9</b>";
    rsh_version_scan_feed(&sc5, p5, sizeof(p5) - 1);
    CHECK(sc5.found && rsh_version_compare(sc5.version, {4, 0, 0}) > 0);

    // Trailer magic.
    uint8_t tail[68] = {};
    memset(tail, 0xAA, 64);
    memcpy(tail + 64, "RSHS", 4);
    CHECK(ota_trailer_has_magic(tail, 68));
    tail[67] = 'X';
    CHECK(!ota_trailer_has_magic(tail, 68));
    CHECK(!ota_trailer_has_magic(tail, 67));
    CHECK(ota_trailer_sig(tail) == tail);

    // Streaming digest (tail holdback): hashed bytes == image[0..N-68],
    // tail == aakhri 68 bytes — chunk split se farq nahi parna chahiye.
    uint8_t simg[300];
    for (int i = 0; i < 300; i++) simg[i] = (uint8_t)(i * 13 + 5);
    RshSignHash sh;
    rec_len = 0;
    rsh_sign_hash_init(&sh, rec_sink, NULL);
    size_t off = 0;
    const size_t splits[] = {1, 7, 68, 69, 100};
    for (size_t s : splits) { rsh_sign_hash_feed(&sh, simg + off, s); off += s; }
    rsh_sign_hash_feed(&sh, simg + off, 300 - off);
    uint8_t tailOut[68];
    size_t tailLen = 0;
    rsh_sign_hash_final(&sh, tailOut, &tailLen);
    CHECK(tailLen == 68);
    CHECK(memcmp(tailOut, simg + 300 - 68, 68) == 0);
    CHECK(rec_len == 300 - 68);
    CHECK(memcmp(rec_buf, simg, 300 - 68) == 0);
    // Chhota upload (< 68 bytes): kuch hash nahi hota, sab tail mein.
    RshSignHash sh2;
    rec_len = 0;
    rsh_sign_hash_init(&sh2, rec_sink, NULL);
    rsh_sign_hash_feed(&sh2, simg, 40);
    uint8_t tail2[68];
    size_t tailLen2 = 0;
    rsh_sign_hash_final(&sh2, tail2, &tailLen2);
    CHECK(tailLen2 == 40 && rec_len == 0 && memcmp(tail2, simg, 40) == 0);
    // Exactly 68 bytes: kuch hash nahi hota, tail = poori image.
    RshSignHash sh3;
    rec_len = 0;
    rsh_sign_hash_init(&sh3, rec_sink, NULL);
    rsh_sign_hash_feed(&sh3, simg, 68);
    uint8_t tail3[68];
    size_t tailLen3 = 0;
    rsh_sign_hash_final(&sh3, tail3, &tailLen3);
    CHECK(tailLen3 == 68 && rec_len == 0 && memcmp(tail3, simg, 68) == 0);

    // Ed25519 — RFC 8032 §7.1 test 1 (empty message): seed se keypair,
    // signature, aur verify — teeno RFC vector se match karein.
    const uint8_t rfc_seed[32] = {0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60,
      0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69,
      0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60};
    const uint8_t rfc_pk[32] = {0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7,
      0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3,
      0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a};
    const uint8_t rfc_sig[64] = {
      0xe5, 0x56, 0x43, 0x00, 0xc3, 0x60, 0xac, 0x72,
      0x90, 0x86, 0xe2, 0xcc, 0x80, 0x6e, 0x82, 0x8a,
      0x84, 0x87, 0x7f, 0x1e, 0xb8, 0xe5, 0xd9, 0x74,
      0xd8, 0x73, 0xe0, 0x65, 0x22, 0x49, 0x01, 0x55,
      0x5f, 0xb8, 0x82, 0x15, 0x90, 0xa3, 0x3b, 0xac,
      0xc6, 0x1e, 0x39, 0x70, 0x1c, 0xf9, 0xb4, 0x6b,
      0xd2, 0x5b, 0xf5, 0xf0, 0x59, 0x5b, 0xbe, 0x24,
      0x65, 0x51, 0x41, 0x43, 0x8e, 0x7a, 0x10, 0x0b};
    uint8_t pk[32], sk[64];
    CHECK(crypto_sign_seed_keypair(pk, sk, rfc_seed) == 0);
    CHECK(memcmp(pk, rfc_pk, 32) == 0);
    const unsigned char dummy = 0;
    uint8_t sig[64];
    unsigned long long siglen = 0;
    CHECK(crypto_sign_detached(sig, &siglen, &dummy, 0, sk) == 0 && siglen == 64);
    CHECK(memcmp(sig, rfc_sig, 64) == 0);
    CHECK(crypto_sign_verify_detached(rfc_sig, &dummy, 0, rfc_pk) == 0);
    CHECK(crypto_sign_verify_detached(rfc_sig, (const unsigned char *)"x", 1, rfc_pk) != 0);

    // Hash-then-sign construction (board verify path): digest = SHA-256(image).
    uint8_t digest[32];
    sha256_once(simg, sizeof(simg), digest);
    uint8_t csig[64];
    unsigned long long clen = 0;
    CHECK(crypto_sign_detached(csig, &clen, digest, 32, sk) == 0);
    CHECK(rsh_verify_signature(csig, digest, pk));
    digest[0] ^= 0xFF;
    CHECK(!rsh_verify_signature(csig, digest, pk));  // badla digest -> fail
    digest[0] ^= 0xFF;
    csig[0] ^= 0x01;
    CHECK(!rsh_verify_signature(csig, digest, pk));  // badli sig -> fail
    csig[0] ^= 0x01;
    uint8_t wrong_pk[32];
    memcpy(wrong_pk, pk, 32);
    wrong_pk[31] ^= 0x01;
    CHECK(!rsh_verify_signature(csig, digest, wrong_pk));  // ghalat key -> fail
    CHECK(!rsh_verify_signature(NULL, digest, pk));
    CHECK(!rsh_verify_signature(csig, NULL, pk));
    CHECK(!rsh_verify_signature(csig, digest, NULL));
  }

  // 21) FW-3/FW-4 dispatcher wiring (follow-up): SET_CFG/COMMIT/CONFIRM/
  // REVERT/GET_CFG/KEY_RESET — asal rsh_ctrl_dispatch se (HMAC + session +
  // cache), rsh_fw34_handler + fake ctx ke saath. net.cpp ka net_ctrl_handler
  // yahi handler delegate karta hai (wahi callbacks, asal millis/NVS/armed).
  {
    rsh_cfg_txn_t txn;
    rsh_cfg_txn_init(&txn);
    uint8_t factory[256];
    size_t flen = rsh_cfg_factory_blob(factory);
    CHECK(rsh_cfg_txn_load_current(&txn, factory, flen) == RSH_CTRL_OK);

    rsh_fw34_handler_ctx_t hctx;
    rsh_fw34_handler_ctx_init(&hctx, &txn);
    hctx.key_reset_armed = fw34_test_armed_cb;
    hctx.key_reset_request = fw34_test_keyreset_cb;
    hctx.cfg_persist = fw34_test_persist_cb;
    hctx.now_ms = fw34_test_now_cb;
    hctx.user = NULL;
    fw34_test_armed = 0;
    fw34_test_now = 1000;
    fw34_test_persist_calls = 0;
    fw34_test_persist_last_pending = 0;
    fw34_test_keyreset_calls = 0;

    rsh_ctrl_session_t sess;
    rsh_ctrl_session_init(&sess);
    rsh_ctrl_cache_t cache;
    rsh_ctrl_cache_init(&cache);
    uint8_t test_key[32], test_nonce[16];  // §19 wale yahan scope mein nahi
    for (int i = 0; i < 32; i++) test_key[i] = (uint8_t)i;
    for (int i = 0; i < 16; i++) test_nonce[i] = (uint8_t)(0xA0 + i);
    uint8_t reply[1200];
    size_t rlen = 0;
    uint32_t ctr = 0;
    uint16_t req = 100;
    rsh_v2_ctrl_t rf;
    auto disp = [&](const uint8_t *fr, size_t fl) {
      rlen = 0;
      return rsh_ctrl_dispatch(fr, fl, test_key, test_nonce, test_hmac16,
                               &sess, &cache, rsh_fw34_handler, &hctx,
                               reply, sizeof(reply), &rlen);
    };
    auto build = [&](uint8_t cmd, const uint8_t *pl, uint16_t pl_len, uint8_t *out) {
      return rsh_v2_ctrl_build(out, 1200, cmd, req++, ctr++, pl, pl_len,
                               test_key, test_nonce, test_hmac16);
    };
    // Reply verify karo; rf mein parsed reply (payload[0] = status).
    auto verified = [&]() {
      return rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf)
             == RSH_V2_OK;
    };
    // SET_CFG chunk payload banao: offset|total|chunkLen|chunkCrc|data.
    auto setcfg_pl = [&](uint16_t off, const uint8_t *blob, uint16_t total,
                         uint16_t clen, uint8_t *out) {
      rsh_put_u16(out + 0, off);
      rsh_put_u16(out + 2, total);
      rsh_put_u16(out + 4, clen);
      rsh_put_u16(out + 6, rsh_crc16_ccitt(blob + off, clen));
      memcpy(out + 8, blob + off, clen);
      return (uint16_t)(8 + clen);
    };
    // Nayi blob (factory jaisi, version N) banao.
    auto blob_v = [&](uint16_t ver, uint8_t *out, size_t *out_len) {
      size_t n = rsh_cfg_factory_blob(out);
      rsh_put_u16(out + 6, ver);
      rsh_put_u32(out + n - 4, rsh_crc32(out, n - 4));
      *out_len = n;
    };

    uint8_t frame[1200];
    uint8_t pl[1100];
    uint8_t newblob[256];
    size_t nlen = 0;
    size_t fl2 = 0;

    // --- Full transaction: out-of-order chunks -> COMMIT (30 s) -> CONFIRM ---
    blob_v(2, newblob, &nlen);
    CHECK(nlen == 188);  // Phase G C3: 11 records (pehle 124 B)
    uint16_t pln = setcfg_pl(0, newblob, (uint16_t)nlen, 63, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 5);
    CHECK(rsh_get_u16(rf.payload + 1) == 63 && rsh_get_u16(rf.payload + 3) == (uint16_t)nlen);
    pln = setcfg_pl(126, newblob, (uint16_t)nlen, 62, pl);  // aakhri chunk pehle
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rsh_get_u16(rf.payload + 1) == 125);
    pln = setcfg_pl(63, newblob, (uint16_t)nlen, 63, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rsh_get_u16(rf.payload + 1) == (uint16_t)nlen);
    CHECK(txn.state == RSH_TXN_STAGED);
    // COMMIT -> OK + confirmDeadlineSec 30 + persist(pending=1).
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    uint8_t commit_frame[1200];
    size_t commit_flen = fl2;
    memcpy(commit_frame, frame, fl2);
    uint32_t calls_before = hctx.calls;
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 2 && rf.payload[1] == 30);
    CHECK(txn.state == RSH_TXN_COMMITTED && txn.confirm_deadline_ms == 1000 + 30000);
    CHECK(fw34_test_persist_calls == 1 && fw34_test_persist_last_pending == 1);
    // Duplicate COMMIT (wahi frame) -> cached reply (pehli jaisi), handler dobara nahi.
    uint8_t first_reply[1200];
    size_t first_rlen = rlen;
    memcpy(first_reply, reply, rlen);
    CHECK(disp(commit_frame, commit_flen) == RSH_CTRL_DISP_REPLY);
    CHECK(rlen == first_rlen && memcmp(reply, first_reply, rlen) == 0);
    CHECK(hctx.calls == calls_before + 1);  // sirf pehla COMMIT count hua
    CHECK(fw34_test_persist_calls == 1);    // dobara persist nahi
    // CONFIRM waqt par (now 30000 < deadline 31000) -> OK + persist(pending=0).
    fw34_test_now = 30000;
    fl2 = build(RSH_CMD_CONFIRM, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && txn.state == RSH_TXN_IDLE);
    CHECK(fw34_test_persist_calls == 2 && fw34_test_persist_last_pending == 0);
    CHECK(memcmp(txn.current, newblob, nlen) == 0);
    // GET_CFG -> totalLen + offset + data (nayi blob wapas).
    uint8_t getpl[4];
    rsh_put_u16(getpl + 0, 0);
    rsh_put_u16(getpl + 2, (uint16_t)nlen);
    fl2 = build(RSH_CMD_GET_CFG, getpl, 4, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 1 + 4 + nlen);
    CHECK(rsh_get_u16(rf.payload + 1) == (uint16_t)nlen && rsh_get_u16(rf.payload + 3) == 0);
    CHECK(memcmp(rf.payload + 5, newblob, nlen) == 0);

    // --- Timeout rollback (dispatcher se): commit v3, deadline par tick ---
    uint8_t blob3[256];
    size_t nlen3 = 0;
    blob_v(3, blob3, &nlen3);
    pln = setcfg_pl(0, blob3, (uint16_t)nlen3, (uint16_t)nlen3, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    CHECK(fw34_test_persist_calls == 3 && fw34_test_persist_last_pending == 1);
    fw34_test_now = 60000;  // deadline = 30000 + 30000
    CHECK(rsh_cfg_tick(&txn, fw34_test_now) == 1);  // loop tick jaisa
    CHECK(memcmp(txn.current, newblob, nlen) == 0);  // v2 wapas (v3 nahi)
    // Der se CONFIRM -> UNSUPPORTED (rollback ho chuka), persist nahi barhta.
    fl2 = build(RSH_CMD_CONFIRM, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_persist_calls == 3);

    // --- REVERT: commit v4 phir revert -> purani (v2) wapas ---
    uint8_t blob4[256];
    size_t nlen4 = 0;
    blob_v(4, blob4, &nlen4);
    pln = setcfg_pl(0, blob4, (uint16_t)nlen4, (uint16_t)nlen4, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    CHECK(fw34_test_persist_calls == 4 && fw34_test_persist_last_pending == 1);
    fl2 = build(RSH_CMD_REVERT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && txn.state == RSH_TXN_IDLE);
    CHECK(memcmp(txn.current, newblob, nlen) == 0);
    CHECK(fw34_test_persist_calls == 5 && fw34_test_persist_last_pending == 0);

    // --- Bad blob COMMIT -> BAD_CRC + err body [errCount=1, errCode=4] ---
    uint8_t badblob[256];
    size_t nlenb = 0;
    blob_v(5, badblob, &nlenb);
    badblob[nlenb - 1] ^= 0xFF;  // CRC32 trailer kharab (chunk CRCs sahi)
    pln = setcfg_pl(0, badblob, (uint16_t)nlenb, (uint16_t)nlenb, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && txn.state == RSH_TXN_STAGED);
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_BAD_CRC && rf.len == 1 + 2);
    CHECK(rf.payload[1] == 1 && rf.payload[2] == RSH_CFG_ERR_CRC_MISMATCH);
    CHECK(txn.state == RSH_TXN_STAGED);  // fix ya REVERT ka mauqa
    CHECK(fw34_test_persist_calls == 5);  // fail par persist nahi
    fl2 = build(RSH_CMD_REVERT, NULL, 0, frame);  // saaf karo
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    CHECK(fw34_test_persist_calls == 6 && fw34_test_persist_last_pending == 0);

    // --- Safe mode: stage + commit BUSY, GET_CFG phir bhi chalta hai ---
    rsh_cfg_set_safe_mode(&txn, 1);
    pln = setcfg_pl(0, newblob, (uint16_t)nlen, 63, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_BUSY);
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_BUSY);
    fl2 = build(RSH_CMD_GET_CFG, getpl, 4, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);  // read block nahi hoti
    rsh_cfg_set_safe_mode(&txn, 0);
    // Chunk CRC ghalat -> BAD_CRC (safe mode clear ke baad bhi).
    uint8_t badcrc_pl[8 + 63];
    pln = setcfg_pl(0, newblob, (uint16_t)nlen, 63, badcrc_pl);
    badcrc_pl[6] ^= 0xFF; badcrc_pl[7] ^= 0xFF;
    fl2 = build(RSH_CMD_SET_CFG, badcrc_pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_BAD_CRC);
    CHECK(rsh_cfg_staged_bytes(&txn) == 0);

    // --- KEY_RESET: unarmed -> UNSUPPORTED; armed -> OK + request ---
    fl2 = build(RSH_CMD_KEY_RESET, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_keyreset_calls == 0);
    fw34_test_armed = 1;
    fl2 = build(RSH_CMD_KEY_RESET, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 1);
    CHECK(fw34_test_keyreset_calls == 1);
    // Armed ho lekin payload khaali na ho -> UNSUPPORTED (request nahi).
    uint8_t onebyte[1] = {0};
    fl2 = build(RSH_CMD_KEY_RESET, onebyte, 1, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_keyreset_calls == 1);
    fw34_test_armed = 0;

    // --- Bad tag SET_CFG -> BAD_TAG, handler bilkul nahi chalta ---
    pln = setcfg_pl(0, newblob, (uint16_t)nlen, 63, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    uint8_t badtag[1200];
    memcpy(badtag, frame, fl2);
    badtag[fl2 - 1] ^= 0xFF;
    uint32_t calls_before_tag = hctx.calls;
    CHECK(disp(badtag, fl2) == RSH_CTRL_DISP_REPLY);
    CHECK(rsh_v2_ctrl_verify(reply, rlen, test_key, test_nonce, test_hmac16, &rf)
          == RSH_V2_OK);
    CHECK(rf.payload[0] == RSH_CTRL_BAD_TAG);
    CHECK(hctx.calls == calls_before_tag);

    // --- Malformed payloads -> UNSUPPORTED (frame sahi, payload ghalat) ---
    // SET_CFG: chunkLen 16 likha, data sirf 8 bytes.
    uint8_t mal_pl[16];
    rsh_put_u16(mal_pl + 0, 0);
    rsh_put_u16(mal_pl + 2, (uint16_t)nlen);
    rsh_put_u16(mal_pl + 4, 16);
    rsh_put_u16(mal_pl + 6, 0);
    memset(mal_pl + 8, 0, 8);
    fl2 = build(RSH_CMD_SET_CFG, mal_pl, 16, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    // GET_CFG: len 2 (4 chahiye).
    uint8_t short_pl[2] = {0, 0};
    fl2 = build(RSH_CMD_GET_CFG, short_pl, 2, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    // COMMIT: payload khaali nahi.
    fl2 = build(RSH_CMD_COMMIT, onebyte, 1, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    // GET_CFG wantLen > 1024 -> TOO_BIG.
    uint8_t bigwant[4];
    rsh_put_u16(bigwant + 0, 0);
    rsh_put_u16(bigwant + 2, 2000);
    fl2 = build(RSH_CMD_GET_CFG, bigwant, 4, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_TOO_BIG);
    // COMMIT jab kuch staged nahi (IDLE) -> UNSUPPORTED.
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    // Unknown cmd (0x7F) -> UNSUPPORTED.
    fl2 = build(0x7F, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(hctx.last_cmd == 0x7F);

    // --- GET_CFG clamp: 2044 B blob, wantLen 1024 -> 1019 data bytes ---
    static uint8_t bigrec[127 * 16];
    for (int i = 0; i < 127; i++)
      rsh_cfg_record(bigrec + i * 16, RSH_CFG_TYPE_LED_STRIP, (uint8_t)i, 0,
                     0xFF, 0xFF, 0xFF, NULL);
    static uint8_t bigblob[2048];
    size_t biglen = rsh_cfg_blob_build(bigblob, bigrec, 127, 9);
    CHECK(biglen == 2044);
    CHECK(rsh_cfg_blob_validate(bigblob, biglen, NULL, NULL) == RSH_CTRL_OK);
    // Frame payload had 1024 hai — chunk <= 1016 (8 header + chunk).
    pln = setcfg_pl(0, bigblob, 2044, 1016, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    pln = setcfg_pl(1016, bigblob, 2044, 1016, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    pln = setcfg_pl(2032, bigblob, 2044, 12, pl);
    fl2 = build(RSH_CMD_SET_CFG, pl, pln, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rsh_get_u16(rf.payload + 1) == 2044);
    CHECK(txn.state == RSH_TXN_STAGED);
    fl2 = build(RSH_CMD_COMMIT, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.payload[1] == 30);
    CHECK(fw34_test_persist_calls == 7 && fw34_test_persist_last_pending == 1);
    fw34_test_now = 61000;  // deadline 60000+30000 ke andar
    fl2 = build(RSH_CMD_CONFIRM, NULL, 0, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK);
    CHECK(fw34_test_persist_calls == 8 && fw34_test_persist_last_pending == 0);
    // GET_CFG offset 0 want 1024 -> 1019 bytes (clamp) + totalLen 2044.
    rsh_put_u16(getpl + 0, 0);
    rsh_put_u16(getpl + 2, 1024);
    fl2 = build(RSH_CMD_GET_CFG, getpl, 4, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 1 + 4 + 1019);
    CHECK(rsh_get_u16(rf.payload + 1) == 2044);
    CHECK(memcmp(rf.payload + 5, bigblob, 1019) == 0);
    // GET_CFG offset 2044 (aakhir) -> 0 data bytes, phir bhi OK.
    rsh_put_u16(getpl + 0, 2044);
    rsh_put_u16(getpl + 2, 8);
    fl2 = build(RSH_CMD_GET_CFG, getpl, 4, frame);
    CHECK(disp(frame, fl2) == RSH_CTRL_DISP_REPLY && verified());
    CHECK(rf.payload[0] == RSH_CTRL_OK && rf.len == 1 + 4);
    CHECK(hctx.last_cmd == RSH_CMD_GET_CFG);
  }

  // 22) SET_PASSWORD (Phase E WS5, PROTOCOL-V2.1.md §5.2, cmd 0x0C):
  // validator + handler, asal rsh_ctrl_dispatch se (HMAC + session).
  // Positive: valid password -> OK + deferred callback ko exact bytes.
  // Negative: length/charset/payload-shape fail -> UNSUPPORTED, callback
  // nahi chalta; bad tag -> BAD_TAG, handler bilkul nahi chalta.
  // NOTE: NVS write + AP restart net.cpp ka kaam hai (Arduino-only) —
  // yahan sirf handler/callback contract test hota hai; real-board
  // verify Phase H A7 mein hoga (AP dobara WPA2 + naya password).
  {
    // --- Validator (password.h): boundaries + charset ---
    const uint8_t pw8[] = {'A', 'b', '2', 'x', 'Y', '9', '!', '#'};  // 8, ok
    const uint8_t pw7[] = {'A', 'b', '2', 'x', 'Y', '9', '!'};       // 7, chhota
    uint8_t pw63[63];
    memset(pw63, 'a', sizeof(pw63));  // 63, ok
    uint8_t pw64[64];
    memset(pw64, 'a', sizeof(pw64));  // 64, bara
    const uint8_t pwSpace[] = {'A', 'b', ' ', 'x', 'Y', '9', '!', '#'};  // space mana
    const uint8_t pwDel[] = {'A', 'b', 0x7F, 'x', 'Y', '9', '!', '#'};   // DEL mana
    const uint8_t pwNul[] = {'A', 'b', 0x00, 'x', 'Y', '9', '!', '#'};   // NUL mana
    const uint8_t pwEdge[] = {'!', '~', 'A', 'z', '2', '9', '0', 'x'};  // 0x21/0x7E ok
    CHECK(rsh_password_validate(pw8, 8) == RSH_CTRL_OK);
    CHECK(rsh_password_validate(pw63, 63) == RSH_CTRL_OK);
    CHECK(rsh_password_validate(pwEdge, 8) == RSH_CTRL_OK);
    CHECK(rsh_password_validate(pw7, 7) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(pw64, 64) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(pwSpace, 8) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(pwDel, 8) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(pwNul, 8) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(NULL, 8) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_password_validate(pw8, 0) == RSH_CTRL_UNSUPPORTED);
    // Generated 24-char password (§5.1) bhi valid hai.
    CHECK(rsh_password_validate((const uint8_t *)"Ab3dEf6hJk9mNp2rSt5vWx8Y", 24)
          == RSH_CTRL_OK);

    // --- Dispatcher: SET_PASSWORD, fake ctx + password callback ---
    rsh_cfg_txn_t txn2;
    rsh_cfg_txn_init(&txn2);
    rsh_fw34_handler_ctx_t hctx2;
    rsh_fw34_handler_ctx_init(&hctx2, &txn2);
    hctx2.password_change_request = fw34_test_password_cb;
    hctx2.user = NULL;
    fw34_test_password_calls = 0;
    fw34_test_password_len = 0;
    rsh_ctrl_session_t sess2;
    rsh_ctrl_session_init(&sess2);
    rsh_ctrl_cache_t cache2;
    rsh_ctrl_cache_init(&cache2);
    uint8_t test_key2[32], test_nonce2[16];
    for (int i = 0; i < 32; i++) test_key2[i] = (uint8_t)i;
    for (int i = 0; i < 16; i++) test_nonce2[i] = (uint8_t)(0xA0 + i);
    uint8_t reply2[1200];
    size_t rlen2 = 0;
    uint32_t ctr2 = 0;
    uint16_t req2 = 500;
    rsh_v2_ctrl_t rf2;
    auto disp2 = [&](const uint8_t *fr, size_t fl) {
      rlen2 = 0;
      return rsh_ctrl_dispatch(fr, fl, test_key2, test_nonce2, test_hmac16,
                               &sess2, &cache2, rsh_fw34_handler, &hctx2,
                               reply2, sizeof(reply2), &rlen2);
    };
    auto build2 = [&](uint8_t cmd, const uint8_t *pl, uint16_t pl_len, uint8_t *out) {
      return rsh_v2_ctrl_build(out, 1200, cmd, req2++, ctr2++, pl, pl_len,
                               test_key2, test_nonce2, test_hmac16);
    };
    auto verified2 = [&]() {
      return rsh_v2_ctrl_verify(reply2, rlen2, test_key2, test_nonce2, test_hmac16,
                                &rf2) == RSH_V2_OK;
    };
    // Payload banao: newLen u8 + password bytes.
    auto pw_pl = [&](const uint8_t *pw, uint8_t n, uint8_t *out) {
      out[0] = n;
      memcpy(out + 1, pw, n);
      return (uint16_t)(1 + n);
    };
    uint8_t frame2[1200];
    uint8_t pl2[128];
    size_t fl22 = 0;
    // Positive: 12-char password -> OK, khaali body, callback ko exact bytes.
    const uint8_t good_pw[] = {'N', 'e', 'w', 'P', 'a', 's', 's', 'w', '0', 'r', 'd', '!'};
    uint16_t pln2 = pw_pl(good_pw, 12, pl2);
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_OK && rf2.len == 1);
    CHECK(fw34_test_password_calls == 1 && fw34_test_password_len == 12);
    CHECK(memcmp(fw34_test_password, good_pw, 12) == 0);
    CHECK(hctx2.last_cmd == RSH_CMD_SET_PASSWORD);
    // Retry (naya counter, wahi password) idempotent hai — dobara callback.
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_OK);
    CHECK(fw34_test_password_calls == 2);
    // Negative: 7 chars -> UNSUPPORTED, callback nahi barhta.
    pln2 = pw_pl(pw7, 7, pl2);
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_password_calls == 2);
    // Negative: newLen 12 likha, payload mein sirf 11 bytes -> UNSUPPORTED.
    pl2[0] = 12;
    memcpy(pl2 + 1, good_pw, 11);
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, 12, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_password_calls == 2);
    // Negative: space wala password -> UNSUPPORTED.
    pln2 = pw_pl(pwSpace, 8, pl2);
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_password_calls == 2);
    // Negative: khaali payload (len 0) -> UNSUPPORTED.
    fl22 = build2(RSH_CMD_SET_PASSWORD, NULL, 0, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_password_calls == 2);
    // Negative: callback NULL ho (fail closed) -> UNSUPPORTED.
    hctx2.password_change_request = NULL;
    pln2 = pw_pl(good_pw, 12, pl2);
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    CHECK(disp2(frame2, fl22) == RSH_CTRL_DISP_REPLY && verified2());
    CHECK(rf2.payload[0] == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_password_calls == 2);
    hctx2.password_change_request = fw34_test_password_cb;
    // Negative: bad tag -> BAD_TAG, handler bilkul nahi chalta.
    fl22 = build2(RSH_CMD_SET_PASSWORD, pl2, pln2, frame2);
    uint8_t badtag2[1200];
    memcpy(badtag2, frame2, fl22);
    badtag2[fl22 - 1] ^= 0xFF;
    uint32_t calls_before2 = hctx2.calls;
    CHECK(disp2(badtag2, fl22) == RSH_CTRL_DISP_REPLY);
    CHECK(rsh_v2_ctrl_verify(reply2, rlen2, test_key2, test_nonce2, test_hmac16, &rf2)
          == RSH_V2_OK);
    CHECK(rf2.payload[0] == RSH_CTRL_BAD_TAG);
    CHECK(hctx2.calls == calls_before2);
    CHECK(fw34_test_password_calls == 2);
  }

  // 23) v2.1 stream emission (Phase E WS5, PROTOCOL-V2.1.md §1.1):
  // faisla (active/v2) + packet build (mV axes, cumulative encoders,
  // module gating). Packet ko wapas rsh_v2_parse_stream se parho —
  // build/parse round-trip hi contract hai.
  {
    // --- raw -> mV scale ---
    CHECK(rsh_raw_to_mv(0) == 0);
    CHECK(rsh_raw_to_mv(4095) == 3300);
    CHECK(rsh_raw_to_mv(2048) == 1650);
    CHECK(rsh_raw_to_mv(4096) == 3300);  // clamp (4096 kabhi aana nahi chahiye)
    // --- faisla (SEC-E-14, Phase F: emission sirf authenticated session ko) ---
    CHECK(rsh_stream_emit_kind(0, 0, 0) == RSH_EMIT_NONE);  // koi PC nahi
    CHECK(rsh_stream_emit_kind(0, 1, 1) == RSH_EMIT_NONE);  // koi PC nahi (auth bhi ho to)
    CHECK(rsh_stream_emit_kind(1, 0, 0) == RSH_EMIT_NONE);  // active lekin unauthenticated — v1 bhi band
    CHECK(rsh_stream_emit_kind(1, 1, 0) == RSH_EMIT_NONE);  // active lekin unauthenticated — v2 bhi band
    CHECK(rsh_stream_emit_kind(1, 0, 1) == RSH_EMIT_V1);    // authenticated legacy peer
    CHECK(rsh_stream_emit_kind(1, 1, 1) == RSH_EMIT_V2);    // authenticated v2 peer

    // Phase F WS5: 5 axes (clutch slot 3 + handbrake slot 4) — sab-on
    // mask 0x1F, packet 36 B (20 + 2*5 + 2*2 + 2). Clutch raw 1024 ->
    // 825 mV, handbrake raw 4095 -> 3300 mV.
    RshModuleEnable enAll = {1, 1, 1, 1, 1, 1, 0};  // sab modules on, clutch on, sequential
    int16_t enc2[4] = {10, -5, 0, 0};
    uint8_t buf[64];
    rsh_v2_stream_t parsed;
    // Sab on: steering mV 1500, accel raw 2048 -> 1650 mV, brake raw 0,
    // clutch raw 1024 -> 825 mV, handbrake raw 4095 -> 3300 mV,
    // encoders, buttons 0x1F.
    size_t n = rsh_build_v2_stream_packet(buf, sizeof(buf), 42, 123456,
                                          RSH_V2_FLAG_AXIS_INVALID, 1500, 1,
                                          2048, 0, 4095, enc2, 2, 0x1F, &enAll,
                                          1024, 0);
    CHECK(n == 20 + 2 * 5 + 2 * 2 + 2);  // 36 B
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.seq == 42 && parsed.timestamp_us == 123456);
    CHECK(parsed.flags == RSH_V2_FLAG_AXIS_INVALID);
    CHECK(parsed.axis_mask == 0x1F);
    CHECK(parsed.axes[0] == 1500 && parsed.axes[1] == 1650);
    CHECK(parsed.axes[2] == 0 && parsed.axes[3] == 825);
    CHECK(parsed.axes[4] == 3300);
    CHECK(parsed.n_enc == 2 && parsed.enc[0] == 10 && parsed.enc[1] == -5);
    CHECK(parsed.buttons == 0x1F);
    // Steering raw mode: 2048 raw -> 1650 mV (convert hota hai).
    n = rsh_build_v2_stream_packet(buf, sizeof(buf), 43, 1000, 0, 2048, 0,
                                   0, 0, 0, enc2, 0, 0, &enAll, 0, 0);
    CHECK(n == 20 + 2 * 5 + 0 + 2);
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.axis_mask == 0x1F && parsed.axes[0] == 1650);
    CHECK(parsed.n_enc == 0);
    // Clutch band: mask 0x17 (slot 3 bit clear), 4 axes.
    RshModuleEnable enNoClutch = {1, 1, 1, 1, 1, 0, 0};
    n = rsh_build_v2_stream_packet(buf, sizeof(buf), 44, 1000, 0, 1500, 1,
                                   2048, 1024, 4095, enc2, 2, 0x1F, &enNoClutch,
                                   2048, 0);
    CHECK(n == 20 + 2 * 4 + 2 * 2 + 2);  // 34 B
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.axis_mask == 0x17);
    CHECK(parsed.axes[0] == 1500 && parsed.axes[1] == 1650 && parsed.axes[2] == 825);
    // Parser slot-index par bharta hai (compact order nahi): slot 3
    // (clutch) maujood nahi -> axes[3] == 0, handbrake slot 4 -> axes[4].
    CHECK(parsed.axes[3] == 0);
    CHECK(parsed.axes[4] == 3300);  // handbrake slot 4 — mask order mein aakhri
    // Handbrake band: mask 0x0F (clutch slot 3 + core), sirf 4 axes.
    RshModuleEnable enNoHb = {0, 1, 1, 1, 1, 1, 0};
    n = rsh_build_v2_stream_packet(buf, sizeof(buf), 45, 1000, 0, 1500, 1,
                                   2048, 1024, 4095, enc2, 2, 0x1F, &enNoHb,
                                   1024, 0);
    CHECK(n == 20 + 2 * 4 + 2 * 2 + 2);  // 34 B
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.axis_mask == 0x0F);
    CHECK(parsed.axes[0] == 1500 && parsed.axes[1] == 1650 && parsed.axes[2] == 825);
    CHECK(parsed.axes[3] == 825);  // clutch slot 3 — mask order mein aakhri
    // H-pattern: gear 2 -> button bit 6 (4+2), up/down bits clear.
    RshModuleEnable enHpat = {1, 1, 1, 1, 1, 1, 1};
    n = rsh_build_v2_stream_packet(buf, sizeof(buf), 46, 1000, 0, 1500, 1,
                                   0, 0, 0, enc2, 0, 0x03, &enHpat, 0, 2);
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.buttons == 0x40);  // sirf gear bit 6
    // Button gating: shifter band -> bits 0,1 clear; horn band -> bit 4 clear.
    RshModuleEnable enNoShiftHorn = {1, 0, 0, 1, 1, 0, 0};
    n = rsh_build_v2_stream_packet(buf, sizeof(buf), 47, 1000, 0, 1500, 1,
                                   0, 0, 0, enc2, 0, 0x1F, &enNoShiftHorn, 0, 0);
    CHECK(rsh_v2_parse_stream(buf, n, &parsed) == RSH_V2_OK);
    CHECK(parsed.buttons == 0x0C);  // sirf accel (bit2) + brake (bit3)
    // Negatives: chhota buffer, n_enc 3, reserved flags.
    CHECK(rsh_build_v2_stream_packet(buf, 10, 1, 1000, 0, 1500, 1, 0, 0, 0,
                                     enc2, 2, 0, &enAll, 0, 0) == 0);
    CHECK(rsh_build_v2_stream_packet(buf, sizeof(buf), 1, 1000, 0, 1500, 1,
                                     0, 0, 0, enc2, 3, 0, &enAll, 0, 0) == 0);
    CHECK(rsh_build_v2_stream_packet(buf, sizeof(buf), 1, 1000, 0x10, 1500, 1,
                                     0, 0, 0, enc2, 0, 0, &enAll, 0, 0) == 0);
    CHECK(rsh_build_v2_stream_packet(NULL, sizeof(buf), 1, 1000, 0, 1500, 1,
                                     0, 0, 0, enc2, 0, 0, &enAll, 0, 0) == 0);
    CHECK(rsh_build_v2_stream_packet(buf, sizeof(buf), 1, 1000, 0, 1500, 1,
                                     0, 0, 0, NULL, 0, 0, &enAll, 0, 0) == 0);
    // 36 B packet 35 B buffer mein nahi banta (5 axes + 2 enc overflow).
    CHECK(rsh_build_v2_stream_packet(buf, 35, 1, 1000, 0, 1500, 1,
                                     2048, 0, 4095, enc2, 2, 0x1F, &enAll,
                                     1024, 0) == 0);
  }

  // 24) NVS password obfuscation (SEC-E-01, Phase F W1, pw_obfuscate.h):
  // XOR keystream = SHA256(key32 || "RSH-PW-V1" || u32le(block)).
  // Round-trip, ciphertext != plaintext, ghalat key se wapas nahi aata,
  // determinism, bounds, fail-closed (SHA fail → plaintext kabhi nahi).
  {
    uint8_t keyA[32], keyB[32];
    for (int i = 0; i < 32; i++) { keyA[i] = (uint8_t)i; keyB[i] = (uint8_t)(0xFF - i); }
    const uint8_t pw24[] = "Ab3dEf6hJk9mNp2rSt5vWx8Y";  // generated 24-char (§5.1)
    const uint8_t pw8[] = {'A', 'b', '2', 'x', 'Y', '9', '!', '#'};
    uint8_t pw63[63];
    memset(pw63, 'a', sizeof(pw63));
    uint8_t ciph[63], back[63], ciph2[63];
    // Round-trip, 3 lambaiyan (8 / 24 / 63 — 63 ko 2 keystream blocks chahiye).
    CHECK(rsh_pw_obfuscate(keyA, pw8, 8, ciph, test_sha256_cb) == 1);
    CHECK(rsh_pw_deobfuscate(keyA, ciph, 8, back, test_sha256_cb) == 1);
    CHECK(memcmp(back, pw8, 8) == 0);
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, ciph, test_sha256_cb) == 1);
    CHECK(rsh_pw_deobfuscate(keyA, ciph, 24, back, test_sha256_cb) == 1);
    CHECK(memcmp(back, pw24, 24) == 0);
    CHECK(rsh_pw_obfuscate(keyA, pw63, 63, ciph, test_sha256_cb) == 1);
    CHECK(rsh_pw_deobfuscate(keyA, ciph, 63, back, test_sha256_cb) == 1);
    CHECK(memcmp(back, pw63, 63) == 0);
    // Ciphertext plaintext nahi hai (flash `strings` se password nahi milta).
    CHECK(memcmp(ciph, pw63, 63) != 0);
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, ciph, test_sha256_cb) == 1);
    CHECK(memcmp(ciph, pw24, 24) != 0);
    // Deterministic: wahi key + wahi password = wahi ciphertext.
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, ciph2, test_sha256_cb) == 1);
    CHECK(memcmp(ciph, ciph2, 24) == 0);
    // Doosri key = doosra ciphertext; ghalat key se deobfuscate karne par
    // asal password wapas nahi aata (aur password.h validation use pakarti hai).
    CHECK(rsh_pw_obfuscate(keyB, pw24, 24, ciph2, test_sha256_cb) == 1);
    CHECK(memcmp(ciph, ciph2, 24) != 0);
    CHECK(rsh_pw_deobfuscate(keyB, ciph, 24, back, test_sha256_cb) == 1);
    CHECK(memcmp(back, pw24, 24) != 0);
    CHECK(rsh_password_validate(back, 24) != RSH_CTRL_OK);
    // Bounds + null: 64 fail, khaali lambai ok (no-op), null/sha-null fail.
    CHECK(rsh_pw_obfuscate(keyA, pw63, 64, ciph, test_sha256_cb) == 0);
    CHECK(rsh_pw_obfuscate(keyA, pw24, 0, ciph, test_sha256_cb) == 1);
    CHECK(rsh_pw_obfuscate(NULL, pw24, 24, ciph, test_sha256_cb) == 0);
    CHECK(rsh_pw_obfuscate(keyA, NULL, 24, ciph, test_sha256_cb) == 0);
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, NULL, test_sha256_cb) == 0);
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, ciph, NULL) == 0);
    // Fail-closed: SHA-256 fail ho to obfuscate fail hota hai — ciphertext
    // buffer ko plaintext kabhi nahi likha jata.
    memset(ciph, 0xAA, sizeof(ciph));
    CHECK(rsh_pw_obfuscate(keyA, pw24, 24, ciph, test_sha256_fail_cb) == 0);
    CHECK(rsh_pw_deobfuscate(keyA, ciph, 24, back, test_sha256_fail_cb) == 0);
  }

  // 25) Phase F WS5 — clutch + shifter gate mode (SET_CFG type 11) +
  // LED_TEST handler (0x0A, §3.7): blob validation/getters, module
  // apply, handler fail-closed cases.
  {
    uint8_t errs[8];
    uint8_t nerr = 0;
    uint8_t recs[3 * 16];
    uint8_t blob[128];

    // --- SHIFTER record (type 11): H-pattern valid ---
    // Gear pins 1..3 = pinA/B/C (13,14,16), 4..6 = params[1..3] (17,18,19),
    // params[0] = mode 1 (H-pattern).
    uint8_t hparams[10] = {1, 17, 18, 19, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, hparams);
    size_t hlen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, hlen, errs, &nerr) == RSH_CTRL_OK);
    CHECK(rsh_cfg_shifter_mode(blob, hlen) == 1);
    // Sequential record (mode 0, pins 0xFF) + encoder record = OK.
    uint8_t sparams[10] = {0, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 0xFF, 0xFF, 0xFF, sparams);
    rsh_cfg_record(recs + 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
    size_t slen = rsh_cfg_blob_build(blob, recs, 2, 1);
    CHECK(rsh_cfg_blob_validate(blob, slen, errs, &nerr) == RSH_CTRL_OK);
    CHECK(rsh_cfg_shifter_mode(blob, slen) == 0);
    // Mode 2 -> PIN_CONFLICT, err OUT_OF_RANGE.
    uint8_t badmode[10] = {2, 17, 18, 19, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, badmode);
    size_t blen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, blen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
    CHECK(errs[0] == RSH_CFG_ERR_OUT_OF_RANGE);
    CHECK(rsh_cfg_shifter_mode(blob, blen) == 0);  // invalid blob -> default 0
    // H-pattern + encoder record -> PIN_CONFLICT (shared pins 16/17/19).
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, hparams);
    rsh_cfg_record(recs + 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
    size_t hclen = rsh_cfg_blob_build(blob, recs, 2, 1);
    CHECK(rsh_cfg_blob_validate(blob, hclen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
    CHECK(errs[0] == RSH_CFG_ERR_PIN_CONFLICT);
    // H-pattern gear pin missing (params[1] = 0xFF) -> PIN_CONFLICT.
    uint8_t missparam[10] = {1, 0xFF, 18, 19, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, missparam);
    size_t mlen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, mlen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
    // Duplicate gear pin (params[1] == pinA 13) -> PIN_CONFLICT.
    uint8_t dupparam[10] = {1, 13, 18, 19, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, dupparam);
    size_t dplen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, dplen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
    CHECK(errs[0] == RSH_CFG_ERR_PIN_CONFLICT);

    // --- CLUTCH record (AXIS_ADC slot 3) ---
    uint8_t pin = 0xFF;
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    size_t clen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, clen, errs, &nerr) == RSH_CTRL_OK);
    CHECK(rsh_cfg_has_clutch(blob, clen, &pin) == 1 && pin == 33);
    // Clutch pin 35 (preset pin): getter pin deta hai; Phase G (C3) se
    // apply bhi karta hai — inputs.cpp ab pinmap se parhta hai, is liye
    // clutch kisi bhi ADC1 pin par chal sakta hai (pehle sirf pin 33).
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 35, 0xFF, 0xFF, NULL);
    size_t c35len = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_has_clutch(blob, c35len, &pin) == 1 && pin == 35);
    // Clutch slot 3 pin 33 + gear-down BUTTON (slot 1) pin 33 -> PIN_CONFLICT.
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 16, RSH_CFG_TYPE_BUTTON, 1, 0, 33, 0xFF, 0xFF, NULL);
    size_t cdlen = rsh_cfg_blob_build(blob, recs, 2, 1);
    CHECK(rsh_cfg_blob_validate(blob, cdlen, errs, &nerr) == RSH_CTRL_PIN_CONFLICT);
    // Factory blob mein clutch nahi, mode sequential hai; LED strip hai.
    uint8_t factory[256];
    size_t flen = rsh_cfg_factory_blob(factory);
    CHECK(rsh_cfg_has_clutch(factory, flen, &pin) == 0);
    CHECK(rsh_cfg_shifter_mode(factory, flen) == 0);
    CHECK(rsh_cfg_led_pin(factory, flen, &pin) == 1 && pin == PIN_LED_STRIP);
    CHECK(rsh_cfg_has_expanders(factory, flen) == 0);

    // --- modules apply_config_blob ---
    rsh_modules_apply_config_blob(factory, flen);
    CHECK(rsh_modules_clutch_enabled() == 0);
    CHECK(rsh_modules_shifter_mode() == 0);
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    clen = rsh_cfg_blob_build(blob, recs, 1, 1);
    rsh_modules_apply_config_blob(blob, clen);
    CHECK(rsh_modules_clutch_enabled() == 1);
    // Clutch pin 35 (preset pin, Phase G C3) -> clutch ON: inputs.cpp
    // pinmap se parhta hai, is liye preset ka clutch pin ab kaam karta hai.
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 35, 0xFF, 0xFF, NULL);
    c35len = rsh_cfg_blob_build(blob, recs, 1, 1);
    rsh_modules_apply_config_blob(blob, c35len);
    CHECK(rsh_modules_clutch_enabled() == 1);
    rsh_cfg_record(recs, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, hparams);
    hlen = rsh_cfg_blob_build(blob, recs, 1, 1);
    rsh_modules_apply_config_blob(blob, hlen);
    CHECK(rsh_modules_shifter_mode() == 1);
    CHECK(rsh_modules_clutch_enabled() == 0);
    // Kharab blob (CRC toda) -> state nahi badalti (fail closed).
    blob[hlen - 1] ^= 0xFF;
    rsh_modules_apply_config_blob(blob, hlen);
    CHECK(rsh_modules_shifter_mode() == 1);
    // Wapas default par restore karo.
    rsh_modules_apply_config_blob(factory, flen);
    CHECK(rsh_modules_clutch_enabled() == 0 && rsh_modules_shifter_mode() == 0);

    // --- LED_TEST handler (0x0A, §3.7) ---
    rsh_fw34_handler_ctx_t lctx;
    rsh_fw34_handler_ctx_init(&lctx, NULL);
    lctx.led_pin = 21;
    lctx.led_has_expanders = 0;
    lctx.led_test_request = fw34_test_led_cb;
    uint8_t reply[64];
    uint16_t reply_len = 0;
    fw34_test_led_calls = 0;
    // Valid SOLID (pattern 1, red, repeats 0 -> 5 s): OK + callback 1 dafa.
    uint8_t solid[8] = {1, 255, 0, 0, 0xE8, 0x03, 0, 0};  // period 1000 ms
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_OK);
    CHECK(fw34_test_led_calls == 1);
    CHECK(fw34_test_led_last.patternId == 1 && fw34_test_led_last.r == 255);
    CHECK(fw34_test_led_last.periodMs == 1000 && fw34_test_led_last.repeats == 0);
    // Callback NULL (production — driver nahi hai) -> UNSUPPORTED, fail closed.
    lctx.led_test_request = NULL;
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    CHECK(fw34_test_led_calls == 1);  // callback dobara nahi chala
    lctx.led_test_request = fw34_test_led_cb;
    // LED module configured nahi (pin 0xFF) -> UNSUPPORTED.
    lctx.led_pin = 0xFF;
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    // R-L9: direct rig mein sirf GPIO21 — pin 25 -> PIN_CONFLICT.
    lctx.led_pin = 25;
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_PIN_CONFLICT);
    // Expanders hon to freed pin (4) OK, 21/22 I2C hain -> PIN_CONFLICT.
    lctx.led_pin = 4;
    lctx.led_has_expanders = 1;
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_OK);
    CHECK(fw34_test_led_calls == 2);
    lctx.led_pin = 21;
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_PIN_CONFLICT);
    lctx.led_pin = 21;
    lctx.led_has_expanders = 0;
    // Unknown patternId 9 -> UNSUPPORTED; reserved != 0 -> UNSUPPORTED.
    uint8_t badpat[8] = {9, 255, 0, 0, 0xE8, 0x03, 0, 0};
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, badpat, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    uint8_t badres[8] = {1, 255, 0, 0, 0xE8, 0x03, 0, 1};
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, badres, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    // BLINK period 50 (< 100) -> UNSUPPORTED; period 100 OK.
    uint8_t blink50[8] = {2, 0, 255, 0, 50, 0, 3, 0};
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, blink50, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    uint8_t blink100[8] = {2, 0, 255, 0, 100, 0, 3, 0};
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, blink100, 8, reply, &reply_len, &lctx)
          == RSH_CTRL_OK);
    CHECK(fw34_test_led_last.patternId == 2 && fw34_test_led_last.repeats == 3);
    // Ghalat length (7 B) -> UNSUPPORTED.
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, solid, 7, reply, &reply_len, &lctx)
          == RSH_CTRL_UNSUPPORTED);
    // LED blob getters: LED_STRIP record + expander record.
    rsh_cfg_record(recs, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
    size_t llen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_led_pin(blob, llen, &pin) == 1 && pin == 21);
    rsh_cfg_record(recs, RSH_CFG_TYPE_LED_STRIP, 0, 0, 4, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 16, RSH_CFG_TYPE_EXP_MCP23017, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
    size_t xlen = rsh_cfg_blob_build(blob, recs, 2, 1);
    CHECK(rsh_cfg_led_pin(blob, xlen, &pin) == 1 && pin == 4);
    CHECK(rsh_cfg_has_expanders(blob, xlen) == 1);
  }

  // 25a) Phase G C3 — pin map board-apply (pinmap.h): blob -> live table,
  // replace semantics, legacy factory blob, GET_CAPS table from blob.
  {
    // --- Defaults = compiled map ---
    RshPinMap def;
    rsh_pinmap_defaults(&def);
    CHECK(def.steer == PIN_STEER && def.accel == PIN_ACCEL_ADC);
    CHECK(def.brake == PIN_BRAKE_ADC && def.handbrake == PIN_HANDBRAKE_ADC);
    CHECK(def.battery == PIN_BATTERY_ADC && def.gearUp == PIN_GEAR_UP);
    CHECK(def.gearDown == PIN_GEAR_DOWN && def.horn == PIN_HORN_BTN);
    CHECK(def.enc1A == PIN_ENC1_A && def.enc1B == PIN_ENC1_B);
    CHECK(def.enc2A == PIN_ENC2_A && def.enc2B == PIN_ENC2_B);
    CHECK(def.ledStrip == PIN_LED_STRIP && def.clutch == RSH_PINMAP_ABSENT);
    CHECK(def.shifterMode == RSH_SHIFTER_MODE);
    CHECK(def.shiftH[0] == PIN_SHIFT_H1 && def.shiftH[5] == PIN_SHIFT_H6);

    // --- Factory blob parse -> poora default map ---
    uint8_t factory[256];
    size_t flen = rsh_cfg_factory_blob(factory);
    RshPinMap fm;
    CHECK(rsh_pinmap_from_blob(factory, flen, &fm) == 1);
    CHECK(fm.steer == 34 && fm.accel == 35 && fm.brake == 36);
    CHECK(fm.handbrake == 32 && fm.battery == 39);
    CHECK(fm.gearUp == 27 && fm.gearDown == 33 && fm.horn == 4);
    CHECK(fm.enc1A == 23 && fm.enc1B == 19 && fm.enc2A == 16 && fm.enc2B == 17);
    CHECK(fm.ledStrip == 21 && fm.clutch == RSH_PINMAP_ABSENT);
    CHECK(fm.shifterMode == 0);

    // --- Custom blob: steering 32, clutch 33, horn/handbrake/enc2/LED absent ---
    uint8_t recs[8 * 16];
    uint8_t blob[256];
    rsh_cfg_record(recs + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 32, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 4 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 5 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 27, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 6 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
    size_t clen = rsh_cfg_blob_build(blob, recs, 7, 1);
    CHECK(rsh_cfg_blob_validate(blob, clen, NULL, NULL) == RSH_CTRL_OK);
    RshPinMap cm;
    CHECK(rsh_pinmap_from_blob(blob, clen, &cm) == 1);
    CHECK(cm.steer == 32 && cm.accel == 35 && cm.brake == 36);
    CHECK(cm.clutch == 33 && cm.battery == 39 && cm.gearUp == 27);
    CHECK(cm.handbrake == RSH_PINMAP_ABSENT && cm.gearDown == RSH_PINMAP_ABSENT);
    CHECK(cm.horn == RSH_PINMAP_ABSENT && cm.ledStrip == RSH_PINMAP_ABSENT);
    CHECK(cm.enc1A == 23 && cm.enc1B == 19);
    CHECK(cm.enc2A == RSH_PINMAP_ABSENT && cm.enc2B == RSH_PINMAP_ABSENT);
    CHECK(cm.shifterMode == 0);
    // Modules presence: clutch on, handbrake/LED/horn off, encoders on (enc1).
    rsh_modules_apply_config_blob(blob, clen);
    CHECK(rsh_modules_clutch_enabled() == 1);
    CHECK(rsh_modules_handbrake_enabled() == 0);
    CHECK(rsh_modules_encoders_enabled() == 1);
    CHECK(rsh_modules_led_enabled() == 0);
    CHECK(rsh_modules_horn_enabled() == 0);
    CHECK(rsh_modules_shifter_enabled() == 1);

    // --- Invalid blob -> 0, out untouched (fail closed) ---
    RshPinMap keep;
    memset(&keep, 0xAA, sizeof(keep));
    uint8_t bad[256];
    memcpy(bad, factory, flen);
    bad[flen - 1] ^= 0xFF;  // CRC32 kharab
    CHECK(rsh_pinmap_from_blob(bad, flen, &keep) == 0);
    CHECK(keep.steer == 0xAA && keep.horn == 0xAA);
    CHECK(rsh_pinmap_from_blob(NULL, flen, &keep) == 0);
    CHECK(rsh_pinmap_from_blob(factory, 0, &keep) == 0);
    CHECK(rsh_pinmap_from_blob(factory, flen, NULL) == 0);

    // --- Legacy factory blob (BUTTON slots 0..3) pehchano ---
    uint8_t lrec[7 * 16];
    rsh_cfg_record(lrec + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 3 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 25, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 4 * 16, RSH_CFG_TYPE_BUTTON, 1, 0, 26, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 5 * 16, RSH_CFG_TYPE_BUTTON, 2, 0, 27, 0xFF, 0xFF, NULL);
    rsh_cfg_record(lrec + 6 * 16, RSH_CFG_TYPE_BUTTON, 3, 0, 33, 0xFF, 0xFF, NULL);
    uint8_t lblob[256];
    size_t llen = rsh_cfg_blob_build(lblob, lrec, 7, 1);
    CHECK(rsh_cfg_blob_validate(lblob, llen, NULL, NULL) == RSH_CTRL_OK);
    CHECK(rsh_pinmap_blob_is_legacy(lblob, llen) == 1);
    CHECK(rsh_pinmap_blob_is_legacy(factory, flen) == 0);  // naya factory
    CHECK(rsh_pinmap_blob_is_legacy(blob, clen) == 0);      // PC-style blob
    CHECK(rsh_pinmap_blob_is_legacy(NULL, 0) == 0);

    // --- Duplicate slot: pehla jeetta hai ---
    uint8_t drec[2 * 16];
    rsh_cfg_record(drec + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
    rsh_cfg_record(drec + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 32, 0xFF, 0xFF, NULL);
    uint8_t dblob[256];
    size_t dlen = rsh_cfg_blob_build(dblob, drec, 2, 1);
    RshPinMap dm;
    CHECK(rsh_pinmap_from_blob(dblob, dlen, &dm) == 1);
    CHECK(dm.steer == 34);
    // --- Aadha encoder (sirf pinA) -> poora encoder absent ---
    rsh_cfg_record(drec + 0 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 0xFF, 0xFF, NULL);
    dlen = rsh_cfg_blob_build(dblob, drec, 1, 1);
    CHECK(rsh_pinmap_from_blob(dblob, dlen, &dm) == 1);
    CHECK(dm.enc1A == RSH_PINMAP_ABSENT && dm.enc1B == RSH_PINMAP_ABSENT);

    // --- Round 2 W3 FIX-2: rsh_pinmap_uses_pin — fixed pedal-button pins
    // (25/26) blob ke kisi aur function ko diye gaye hon to 1 (inputs.cpp
    // unhein fixed button ke tor par parhna band karta hai — warna ek hi
    // pin do button bits deta). Factory blob mein 25/26 kisi ko nahi.
    {
      RshPinMap fm;
      CHECK(rsh_pinmap_from_blob(factory, flen, &fm) == 1);
      CHECK(rsh_pinmap_uses_pin(&fm, 25) == 0);
      CHECK(rsh_pinmap_uses_pin(&fm, 26) == 0);
      CHECK(rsh_pinmap_uses_pin(&fm, 34) == 1);   // steering
      CHECK(rsh_pinmap_uses_pin(&fm, 21) == 1);   // LED strip
      CHECK(rsh_pinmap_uses_pin(&fm, 0xFF) == 0);
      CHECK(rsh_pinmap_uses_pin(NULL, 25) == 0);
      // Full-rig style: encoder 1 pins 23/25 — pin 25 istemal mein hai.
      rsh_cfg_record(drec + 0 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 25, 0xFF, NULL);
      dlen = rsh_cfg_blob_build(dblob, drec, 1, 1);
      RshPinMap em;
      CHECK(rsh_pinmap_from_blob(dblob, dlen, &em) == 1);
      CHECK(rsh_pinmap_uses_pin(&em, 25) == 1);
      CHECK(rsh_pinmap_uses_pin(&em, 23) == 1);
      CHECK(rsh_pinmap_uses_pin(&em, 26) == 0);
      CHECK(rsh_pinmap_uses_pin(&em, 27) == 0);
    }

    // --- H-pattern blob: mode 1, gear pins, encoders off ---
    uint8_t hparams[10] = {1, 17, 18, 19, 0, 0, 0, 0, 0, 0};
    rsh_cfg_record(recs + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 3 * 16, RSH_CFG_TYPE_SHIFTER, 0, 0, 13, 14, 16, hparams);
    size_t hlen = rsh_cfg_blob_build(blob, recs, 4, 1);
    CHECK(rsh_cfg_blob_validate(blob, hlen, NULL, NULL) == RSH_CTRL_OK);
    RshPinMap hm;
    CHECK(rsh_pinmap_from_blob(blob, hlen, &hm) == 1);
    CHECK(hm.shifterMode == 1);
    CHECK(hm.shiftH[0] == 13 && hm.shiftH[1] == 14 && hm.shiftH[2] == 16);
    CHECK(hm.shiftH[3] == 17 && hm.shiftH[4] == 18 && hm.shiftH[5] == 19);
    rsh_modules_apply_config_blob(blob, hlen);
    CHECK(rsh_modules_shifter_mode() == 1);
    CHECK(rsh_modules_shifter_enabled() == 1);
    CHECK(rsh_modules_encoders_enabled() == 0);  // H-pattern mein encoders nahi

    // --- GET_CAPS table factory blob se == default table (entry by entry) ---
    RshCapsEntry defTable[15];
    RshCapsEntry blobTable[40];
    CHECK(rsh_caps_default_table(defTable, 15) == 15);
    CHECK(rsh_caps_table_from_pinmap(&fm, blobTable, 40) == 15);
    for (int i = 0; i < 15; i++) {
      CHECK(blobTable[i].pin == defTable[i].pin);
      CHECK(blobTable[i].kind == defTable[i].kind);
      CHECK(blobTable[i].module == defTable[i].module);
      CHECK(blobTable[i].caps == defTable[i].caps);
    }
    // --- GET_CAPS table custom blob se: 10 rows, horn/enc2/LED nahi ---
    RshPinMap cm2;
    // Custom blob dobara banao (blob mein ab H-pattern blob hai).
    rsh_cfg_record(recs + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 32, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 4 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 5 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 27, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 6 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
    clen = rsh_cfg_blob_build(blob, recs, 7, 1);
    CHECK(rsh_pinmap_from_blob(blob, clen, &cm2) == 1);
    uint8_t ntab = rsh_caps_table_from_pinmap(&cm2, blobTable, 40);
    CHECK(ntab == 10);  // steer/accel/brake/clutch/battery/gearUp + 2 fixed btn + enc1A/B
    CHECK(blobTable[0].pin == 32 && blobTable[0].module == RSH_MOD_STEERING);
    CHECK(blobTable[3].pin == 33 && blobTable[3].module == RSH_MOD_CLUTCH);
    CHECK(blobTable[4].pin == 39 && blobTable[4].module == RSH_MOD_BATTERY);
    CHECK(blobTable[5].pin == 27 && blobTable[5].module == RSH_MOD_SHIFTER);
    bool hasHorn = false, hasLed = false, hasEnc2 = false;
    for (int i = 0; i < ntab; i++) {
      if (blobTable[i].module == RSH_MOD_HORN) hasHorn = true;
      if (blobTable[i].module == RSH_MOD_LED) hasLed = true;
      if (blobTable[i].pin == 16 || blobTable[i].pin == 17) hasEnc2 = true;
    }
    CHECK(!hasHorn && !hasLed && !hasEnc2);
    // H-pattern table: 3 axes + 6 gear rows + 2 fixed buttons = 11.
    CHECK(rsh_caps_table_from_pinmap(&hm, blobTable, 40) == 11);
    CHECK(blobTable[3].pin == 13 && blobTable[3].module == RSH_MOD_SHIFTER);
    CHECK(blobTable[7].pin == 18 && blobTable[7].module == RSH_MOD_SHIFTER);
    CHECK(blobTable[8].pin == 19 && blobTable[8].module == RSH_MOD_SHIFTER);
    // NULL / chhota out -> 0.
    CHECK(rsh_caps_table_from_pinmap(NULL, blobTable, 40) == 0);
    CHECK(rsh_caps_table_from_pinmap(&fm, NULL, 40) == 0);
    CHECK(rsh_caps_table_from_pinmap(&fm, blobTable, 5) == 0);

    // Wapas default par restore karo (factory blob — sab on, clutch off).
    rsh_modules_apply_config_blob(factory, flen);
    CHECK(rsh_modules_clutch_enabled() == 0 && rsh_modules_shifter_mode() == 0);
    CHECK(rsh_modules_handbrake_enabled() == 1);
    CHECK(rsh_modules_encoders_enabled() == 1);
    CHECK(rsh_modules_led_enabled() == 1);
    CHECK(rsh_modules_horn_enabled() == 1);
  }

  // 26) Phase G WS5 — board-side verify (sandbox): encoder 200-turn
  // stress + i16 wrap + bounce, shifter gate state machine, clutch
  // slot sweep (GPIO33 path), LED driver abstraction (led_driver.h).
  {
    const uint8_t cyc[4][2] = {{0, 0}, {1, 0}, {1, 1}, {0, 1}};

    // --- Encoder 200-turn stress: 200 aage (+800) + 200 peeche = exact 0.
    // FW-12 gate 100 turns tha — ye us ka double hai (200 turns 0 drift).
    RshEncoder e200;
    rsh_encoder_init(&e200);
    rsh_encoder_step(&e200, 0, 0);
    for (int turn = 0; turn < 200; turn++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&e200, cyc[s % 4][0], cyc[s % 4][1]);
    CHECK(e200.count == 800);
    CHECK(e200.glitches == 0);  // clean rotation mein koi glitch nahi
    for (int turn = 0; turn < 200; turn++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&e200, cyc[(4 - s) % 4][0], cyc[(4 - s) % 4][1]);
    CHECK(e200.count == 0);

    // --- i16 wrap stress: wrap boundary ke paar ghumao, wapas aao.
    // 32760 + 40 steps -> 32800 wraps to -32736; delta phir bhi +40.
    RshEncoder w;
    rsh_encoder_init(&w);
    w.count = 32760;
    rsh_encoder_step(&w, 0, 0);
    for (int t = 0; t < 10; t++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&w, cyc[s % 4][0], cyc[s % 4][1]);
    CHECK(w.count == -32736);
    CHECK(rsh_encoder_delta(w.count, 32760) == 40);
    for (int t = 0; t < 10; t++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&w, cyc[(4 - s) % 4][0], cyc[(4 - s) % 4][1]);
    CHECK(w.count == 32760);
    CHECK(rsh_encoder_delta(w.count, -32736) == -40);
    // Neeche wali boundary: -32760 - 40 -> -32800 wraps to 32736.
    RshEncoder wmin;
    rsh_encoder_init(&wmin);
    wmin.count = -32760;
    rsh_encoder_step(&wmin, 0, 0);
    for (int t = 0; t < 10; t++)
      for (int s = 1; s <= 4; s++) rsh_encoder_step(&wmin, cyc[(4 - s) % 4][0], cyc[(4 - s) % 4][1]);
    CHECK(wmin.count == 32736);
    CHECK(rsh_encoder_delta(wmin.count, -32760) == -40);

    // --- Bounce/reversal stress (seeded LCG — deterministic): 300 rounds,
    // har round 1-4 clean cycles (aage ya peeche) + 0-2 bounce pairs
    // (ek qadam aage + foran wapas). Bounce pair ka net 0 hona chahiye —
    // aakhri count sirf clean cycles ka hona chahiye.
    RshEncoder eb;
    rsh_encoder_init(&eb);
    rsh_encoder_step(&eb, 0, 0);
    uint8_t idx = 0;  // cyc mein maujooda state (0..3)
    int expected = 0;
    int bouncePairs = 0;
    uint32_t lcg = 12345;
    for (int round = 0; round < 300; round++) {
      lcg = lcg * 1103515245u + 12345u;
      int fwd = (int)((lcg >> 16) & 1u);
      lcg = lcg * 1103515245u + 12345u;
      int cycles = 1 + (int)((lcg >> 16) % 4u);
      for (int c = 0; c < cycles; c++) {
        for (int s = 0; s < 4; s++) {
          idx = (uint8_t)((idx + (fwd ? 1 : 3)) & 3);
          rsh_encoder_step(&eb, cyc[idx][0], cyc[idx][1]);
        }
        expected += fwd ? 4 : -4;
      }
      lcg = lcg * 1103515245u + 12345u;
      int bounces = (int)((lcg >> 16) % 3u);
      for (int bp = 0; bp < bounces; bp++) {
        // Ek qadam aage (kisi bhi disha), foran wapas — net 0.
        uint8_t back = idx;
        idx = (uint8_t)((idx + 1) & 3);
        rsh_encoder_step(&eb, cyc[idx][0], cyc[idx][1]);
        idx = back;
        rsh_encoder_step(&eb, cyc[idx][0], cyc[idx][1]);
        bouncePairs++;
      }
    }
    CHECK(eb.count == (int16_t)expected);
    CHECK(bouncePairs > 0);  // bounce waqai inject hue
    CHECK(eb.glitches >= (uint32_t)bouncePairs);  // har bounce pair 1 glitch
    // --- Invalid-transition burst: dono bits ek saath 500 dafa — count 0.
    RshEncoder eg;
    rsh_encoder_init(&eg);
    rsh_encoder_step(&eg, 0, 0);
    for (int i = 0; i < 500; i++) {
      rsh_encoder_step(&eg, 1, 1);  // 00 -> 11
      CHECK(eg.count == 0);
      rsh_encoder_step(&eg, 0, 0);  // 11 -> 00
      CHECK(eg.count == 0);
    }
    CHECK(eg.glitches == 1000);
    // Glitch ke baad decoder recover karta hai: ek clean cycle = +4.
    for (int s = 1; s <= 4; s++) rsh_encoder_step(&eg, cyc[s % 4][0], cyc[s % 4][1]);
    CHECK(eg.count == 4);

    // --- Shifter gate state machine: sequential <-> H-pattern switch.
    // Sequential: up/down bits; gear value nazar-andaz hoti hai.
    RshModuleEnable enSeq = {1, 1, 1, 1, 1, 0, 0};
    CHECK(rsh_stream_button_bits(&enSeq, 1, 0, 0, 0, 0, 3) == 0x01);
    CHECK(rsh_stream_button_bits(&enSeq, 0, 1, 0, 0, 0, 6) == 0x02);
    CHECK(rsh_stream_button_bits(&enSeq, 1, 1, 0, 0, 0, 0) == 0x03);
    // H-pattern: sirf gear bits 5..10; up/down nazar-andaz hote hain.
    RshModuleEnable enH = {1, 1, 1, 1, 1, 0, 1};
    CHECK(rsh_stream_button_bits(&enH, 1, 1, 0, 0, 0, 0) == 0x00);
    for (uint8_t g = 1; g <= 6; g++)
      CHECK(rsh_stream_button_bits(&enH, 0, 0, 0, 0, 0, g) == (1u << (4u + g)));
    CHECK(rsh_stream_button_bits(&enH, 0, 0, 0, 0, 0, 7) == 0x00);  // gear 7 invalid
    // Gear sequence 1->6->neutral, beech mein invalid (2 bits) -> neutral,
    // phir decoder foran recover (stateless — agli gear sahi).
    CHECK(rsh_shifter_hpattern_gear(0x01) == 1);
    CHECK(rsh_shifter_hpattern_gear(0x02) == 2);
    CHECK(rsh_shifter_hpattern_gear(0x04) == 3);
    CHECK(rsh_shifter_hpattern_gear(0x08) == 4);
    CHECK(rsh_shifter_hpattern_gear(0x10) == 5);
    CHECK(rsh_shifter_hpattern_gear(0x20) == 6);
    CHECK(rsh_shifter_hpattern_gear(0x00) == 0);
    CHECK(rsh_shifter_hpattern_gear(0x03) == 0);  // gear 1+2 ek saath — invalid
    CHECK(rsh_shifter_hpattern_gear(0x10) == 5);  // recover
    // Packet level par mode switch: wahi physical inputs, alag bits.
    // Sequential packet: up+down dabaye -> buttons 0x03.
    int16_t enc0[4] = {0, 0, 0, 0};
    uint8_t pbuf[64];
    rsh_v2_stream_t pp;
    size_t pn = rsh_build_v2_stream_packet(pbuf, sizeof(pbuf), 1, 1000, 0, 1500, 1,
                                           0, 0, 0, enc0, 0, 0x03, &enSeq, 0, 0);
    CHECK(pn > 0 && rsh_v2_parse_stream(pbuf, pn, &pp) == RSH_V2_OK);
    CHECK(pp.buttons == 0x03);
    // H-pattern packet: up/down dabaye + gear 3 -> sirf gear bit 7 (0x80).
    pn = rsh_build_v2_stream_packet(pbuf, sizeof(pbuf), 2, 1000, 0, 1500, 1,
                                    0, 0, 0, enc0, 0, 0x03, &enH, 0, 3);
    CHECK(pn > 0 && rsh_v2_parse_stream(pbuf, pn, &pp) == RSH_V2_OK);
    CHECK(pp.buttons == 0x80);
    // H-pattern + encoders ek saath: conflict (shared pins 16/17/19) —
    // is halat mein encoders report 0 hote hain (inputs.cpp else branch).
    CHECK(rsh_modules_hpattern_encoder_conflict(RSH_SHIFTER_HPATTERN, 1) == 1);
    pn = rsh_build_v2_stream_packet(pbuf, sizeof(pbuf), 3, 1000, 0, 1500, 1,
                                    0, 0, 0, enc0, 0, 0x00, &enH, 0, 0);
    CHECK(pn > 0 && rsh_v2_parse_stream(pbuf, pn, &pp) == RSH_V2_OK);
    CHECK(pp.n_enc == 0);  // H-pattern mode mein encoders nahi bheje jate

    // --- Clutch board-side path (GPIO33, slot 3): raw -> mV sweep.
    // inputs.cpp clutch on hone par avg_pedal(PIN_CLUTCH_ADC) parhta hai;
    // yahan pure path (raw counts -> stream packet slot 3) verify hota hai.
    RshModuleEnable enC = {1, 1, 1, 1, 1, 1, 0};  // clutch on
    const uint16_t craw[5] = {0, 1024, 2048, 3072, 4095};
    const uint16_t cmv[5] = {0, 825, 1650, 2476, 3300};
    for (int i = 0; i < 5; i++) {
      pn = rsh_build_v2_stream_packet(pbuf, sizeof(pbuf), 10, 1000, 0, 1500, 1,
                                      0, 0, 0, enc0, 0, 0x00, &enC, craw[i], 0);
      CHECK(pn > 0 && rsh_v2_parse_stream(pbuf, pn, &pp) == RSH_V2_OK);
      CHECK(pp.axis_mask == 0x1F);
      CHECK(pp.axes[3] == cmv[i]);  // clutch slot 3
    }
    // Clutch band: raw 4095 bhi slot 3 mein nahi jata (mask bit clear).
    RshModuleEnable enNoC = {1, 1, 1, 1, 1, 0, 0};
    pn = rsh_build_v2_stream_packet(pbuf, sizeof(pbuf), 11, 1000, 0, 1500, 1,
                                    0, 0, 4095, enc0, 0, 0x00, &enNoC, 4095, 0);
    CHECK(pn > 0 && rsh_v2_parse_stream(pbuf, pn, &pp) == RSH_V2_OK);
    CHECK(pp.axis_mask == 0x17);
    CHECK(pp.axes[3] == 0);       // clutch slot khaali
    CHECK(pp.axes[4] == 3300);    // handbrake slot 4 phir bhi maujood
    // Clutch on/off mask transitions (F7): 0x17 <-> 0x1F.
    CHECK(rsh_stream_axis_mask(&enNoC) == 0x17);
    CHECK(rsh_stream_axis_mask(&enC) == 0x1F);

    // --- LED driver abstraction (led_driver.h): init/start/tick/stop.
    RshLedDriver bad;
    CHECK(rsh_led_driver_init(NULL, 16, led_sink_rec, NULL) == 0);
    CHECK(rsh_led_driver_init(&bad, 0, led_sink_rec, NULL) == 0);
    CHECK(rsh_led_driver_init(&bad, 61, led_sink_rec, NULL) == 0);
    CHECK(rsh_led_driver_init(&bad, 16, NULL, NULL) == 0);
    CHECK(rsh_led_driver_init(&bad, 16, led_sink_rec, NULL) == 1);
    CHECK(rsh_led_driver_active(&bad) == 0);
    CHECK(rsh_led_driver_done(&bad, 0) == 1);
    CHECK(rsh_led_driver_tick(&bad, 0) == 0);  // inactive — kuch nahi
    CHECK(rsh_led_driver_stop(&bad) == 0);
    // Start validation: ghalat pattern chalta hua pattern nahi bigarta.
    RshLedPattern solid = {RSH_LED_SOLID, 255, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&bad, &solid, 1000, 21, 0, 1) == RSH_CTRL_OK);
    CHECK(rsh_led_driver_active(&bad) == 1);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&bad, 1000) == 1);  // SOLID frame ek dafa
    CHECK(led_sink_writes == 1 && led_sink_counts[0] == 16);
    CHECK(led_sink_frames[0][0].r == 255 && led_sink_frames[0][0].g == 0);
    CHECK(led_sink_frames[0][15].r == 255);       // aakhri pixel bhi laal
    CHECK(rsh_led_driver_tick(&bad, 1001) == 0);  // frame waisa hi — no write
    CHECK(rsh_led_driver_tick(&bad, 5999) == 0);
    CHECK(led_sink_writes == 1);  // SOLID ne 5 s mein sirf 1 write kiya
    RshLedPattern badpat = {9, 0, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&bad, &badpat, 2000, 21, 0, 1) == RSH_CTRL_UNSUPPORTED);
    CHECK(rsh_led_driver_active(&bad) == 1);     // SOLID abhi bhi chal raha hai
    CHECK(rsh_led_driver_tick(&bad, 2000) == 0);  // frame waisa hi
    CHECK(rsh_led_driver_done(&bad, 5999) == 0);
    CHECK(rsh_led_driver_tick(&bad, 6000) == 1);  // done -> all-off ek dafa
    CHECK(led_sink_writes == 2);
    CHECK(led_sink_frames[1][0].r == 0 && led_sink_frames[1][15].b == 0);
    CHECK(rsh_led_driver_active(&bad) == 0);
    CHECK(rsh_led_driver_tick(&bad, 6001) == 0);  // done ke baad khamosh
    // BLINK 1000 ms, 4 LEDs: on/off har 500 ms — 5 s mein 10 writes.
    RshLedDriver bl;
    CHECK(rsh_led_driver_init(&bl, 4, led_sink_rec, NULL) == 1);
    RshLedPattern blink = {RSH_LED_BLINK, 0, 255, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&bl, &blink, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&bl, 0) == 1);     // on
    CHECK(led_sink_frames[0][0].g == 255);
    CHECK(rsh_led_driver_tick(&bl, 250) == 0);   // wahi phase — no write
    CHECK(rsh_led_driver_tick(&bl, 500) == 1);   // off
    CHECK(led_sink_frames[1][0].g == 0);
    CHECK(rsh_led_driver_tick(&bl, 999) == 0);
    CHECK(rsh_led_driver_tick(&bl, 1000) == 1);  // on dobara
    CHECK(led_sink_frames[2][0].g == 255);
    for (uint32_t t = 1250; t < 5000; t += 250) (void)rsh_led_driver_tick(&bl, t);
    CHECK(led_sink_writes == 10);
    CHECK(rsh_led_driver_tick(&bl, 5000) == 0);  // done, pehle se off — no write
    CHECK(rsh_led_driver_active(&bl) == 0);
    // CHASE 400 ms, 4 LEDs: lit pixel har 100 ms badalta hai.
    RshLedDriver ch;
    CHECK(rsh_led_driver_init(&ch, 4, led_sink_rec, NULL) == 1);
    RshLedPattern chase = {RSH_LED_CHASE, 0, 0, 255, 400, 0, 0};
    CHECK(rsh_led_driver_start(&ch, &chase, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&ch, 0) == 1);
    CHECK(led_sink_frames[0][0].b == 255 && led_sink_frames[0][1].b == 0);
    CHECK(rsh_led_driver_tick(&ch, 50) == 0);
    CHECK(rsh_led_driver_tick(&ch, 150) == 1);
    CHECK(led_sink_frames[1][1].b == 255 && led_sink_frames[1][0].b == 0);
    CHECK(rsh_led_driver_tick(&ch, 350) == 1);
    CHECK(led_sink_frames[2][3].b == 255);
    CHECK(rsh_led_driver_tick(&ch, 400) == 1);   // cycle dobara — pixel 0
    CHECK(led_sink_frames[3][0].b == 255);
    // RAINBOW: waqt ke saath rang badalta hai (har tick naya frame).
    RshLedDriver rb;
    CHECK(rsh_led_driver_init(&rb, 1, led_sink_rec, NULL) == 1);
    RshLedPattern rainbow = {RSH_LED_RAINBOW, 0, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&rb, &rainbow, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&rb, 0) == 1);
    CHECK(led_sink_frames[0][0].r == 255 && led_sink_frames[0][0].g == 0);
    CHECK(rsh_led_driver_tick(&rb, 250) == 1);
    CHECK(!(led_sink_frames[1][0].r == led_sink_frames[0][0].r &&
            led_sink_frames[1][0].g == led_sink_frames[0][0].g &&
            led_sink_frames[1][0].b == led_sink_frames[0][0].b));
    // OFF: foran done, all-off ek dafa.
    RshLedDriver off;
    CHECK(rsh_led_driver_init(&off, 4, led_sink_rec, NULL) == 1);
    RshLedPattern offpat = {RSH_LED_OFF, 0, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&off, &offpat, 0, 21, 0, 1) == RSH_CTRL_OK);
    CHECK(rsh_led_driver_done(&off, 0) == 1);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&off, 0) == 1);
    CHECK(led_sink_frames[0][0].r == 0);
    CHECK(rsh_led_driver_tick(&off, 1) == 0);
    // Repeats: BLINK period 1000 repeats 2 -> 2000 ms par done.
    RshLedDriver rp;
    CHECK(rsh_led_driver_init(&rp, 4, led_sink_rec, NULL) == 1);
    RshLedPattern blink2 = {RSH_LED_BLINK, 255, 255, 255, 1000, 2, 0};
    CHECK(rsh_led_driver_start(&rp, &blink2, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&rp, 0) == 1);
    CHECK(rsh_led_driver_tick(&rp, 500) == 1);
    CHECK(rsh_led_driver_tick(&rp, 1000) == 1);
    CHECK(rsh_led_driver_tick(&rp, 1500) == 1);
    CHECK(rsh_led_driver_done(&rp, 1999) == 0);
    CHECK(rsh_led_driver_tick(&rp, 2000) == 0);  // done, pehle se off
    CHECK(led_sink_writes == 4);
    // stop(): chalta hua SOLID roko — all-off ek dafa, phir khamosh.
    RshLedDriver st;
    CHECK(rsh_led_driver_init(&st, 4, led_sink_rec, NULL) == 1);
    CHECK(rsh_led_driver_start(&st, &solid, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&st, 0) == 1);
    CHECK(rsh_led_driver_stop(&st) == 1);
    CHECK(led_sink_writes == 2);
    CHECK(led_sink_frames[1][0].r == 0);
    CHECK(rsh_led_driver_active(&st) == 0);
    CHECK(rsh_led_driver_stop(&st) == 0);   // dobara stop — kuch nahi
    CHECK(rsh_led_driver_tick(&st, 10) == 0);
    // R-L9: driver start bhi pin rule lagata hai (pin 25 -> PIN_CONFLICT).
    RshLedDriver pc;
    CHECK(rsh_led_driver_init(&pc, 4, led_sink_rec, NULL) == 1);
    CHECK(rsh_led_driver_start(&pc, &solid, 0, 25, 0, 1) == RSH_CTRL_PIN_CONFLICT);
    CHECK(rsh_led_driver_active(&pc) == 0);
    // Handler -> driver chain: LED_TEST dispatch (handler) pattern deta hai,
    // driver us se frames banata hai (yahi rasta Phase H mein net.cpp mein
    // hoga — led_test_request adapter). Production mein callback NULL hai
    // to handler UNSUPPORTED deta hai (fail closed — §25 mein covered).
    rsh_fw34_handler_ctx_t hctx;
    rsh_fw34_handler_ctx_init(&hctx, NULL);
    hctx.led_pin = 21;
    hctx.led_has_expanders = 0;
    hctx.led_test_request = fw34_test_led_cb;
    fw34_test_led_calls = 0;
    uint8_t hreply[64];
    uint16_t hreply_len = 0;
    uint8_t hpat[8] = {1, 255, 0, 0, 0xE8, 0x03, 0, 0};  // SOLID red, 1000 ms
    CHECK(rsh_fw34_handler(RSH_CMD_LED_TEST, hpat, 8, hreply, &hreply_len, &hctx)
          == RSH_CTRL_OK);
    CHECK(fw34_test_led_calls == 1);
    RshLedDriver hd;
    CHECK(rsh_led_driver_init(&hd, 4, led_sink_rec, NULL) == 1);
    CHECK(rsh_led_driver_start(&hd, &fw34_test_led_last, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&hd, 0) == 1);
    CHECK(led_sink_frames[0][0].r == 255 && led_sink_frames[0][3].r == 255);
    CHECK(rsh_led_driver_tick(&hd, 4999) == 0);
    CHECK(rsh_led_driver_tick(&hd, 5000) == 1);  // 5 s -> all-off
    CHECK(led_sink_frames[1][0].r == 0);
  }

  // 27) Phase H WS2 — WS2812 RMT sink (ws2812_rmt.h) + SEC-H-10 cap.
  // Encode pure hai (Arduino-free): bitstream, GRB order, timing aur
  // frame size host par assert hote hain. Asal RMT channel setup +
  // transmit (led_sink_rmt.cpp) sirf board par verify ho sakta hai —
  // A7 §25 mein aankh se (strip ki roshni).
  {
    // --- Guards: NULL / 0 / > 60 / capacity kam -> 0 items.
    RshWs2812Item items[RSH_WS2812_MAX_ITEMS];
    uint8_t px1[3] = {0x11, 0x22, 0x33};  // r, g, b
    CHECK(rsh_ws2812_encode(NULL, 1, items, RSH_WS2812_MAX_ITEMS) == 0);
    CHECK(rsh_ws2812_encode(px1, 1, NULL, RSH_WS2812_MAX_ITEMS) == 0);
    CHECK(rsh_ws2812_encode(px1, 0, items, RSH_WS2812_MAX_ITEMS) == 0);
    CHECK(rsh_ws2812_encode(px1, 61, items, RSH_WS2812_MAX_ITEMS) == 0);
    CHECK(rsh_ws2812_encode(px1, 1, items, 24) == 0);  // 25 chahiye
    // --- Timing constants datasheet range mein (WS2812B, ns).
    CHECK(RSH_WS2812_T0H_NS == 400 && RSH_WS2812_T0H_NS >= 250 &&
          RSH_WS2812_T0H_NS <= 550);
    CHECK(RSH_WS2812_T0L_NS == 800 && RSH_WS2812_T0L_NS >= 700 &&
          RSH_WS2812_T0L_NS <= 1000);
    CHECK(RSH_WS2812_T1H_NS == 800 && RSH_WS2812_T1H_NS >= 650 &&
          RSH_WS2812_T1H_NS <= 950);
    CHECK(RSH_WS2812_T1L_NS == 400 && RSH_WS2812_T1L_NS >= 300 &&
          RSH_WS2812_T1L_NS <= 600);
    CHECK(RSH_WS2812_T0H_NS + RSH_WS2812_T0L_NS == 1200);
    CHECK(RSH_WS2812_T1H_NS + RSH_WS2812_T1L_NS == 1200);
    CHECK(RSH_WS2812_RESET_TICKS * RSH_WS2812_TICK_NS >= 50000);
    // --- Frame time budget: 16 LEDs ~511 us, 60 LEDs ~1778 us
    // (200 Hz loop budget 3500 us se neeche; RMT hardware bhejta hai).
    CHECK(rsh_ws2812_frame_us(16) == 510);
    CHECK(rsh_ws2812_frame_us(60) == 1778);
    CHECK(rsh_ws2812_frame_us(1) == 78);
    CHECK(rsh_ws2812_item_count(16) == 385);
    CHECK(rsh_ws2812_item_count(60) == 1441);
    // --- 1 LED {r=0x11, g=0x22, b=0x33}: 25 items (24 bits + reset).
    // GRB order: pehle G (0x22 = 00100010), phir R (0x11 = 00010001),
    // phir B (0x33 = 00110011); har byte MSB-first.
    CHECK(rsh_ws2812_encode(px1, 1, items, RSH_WS2812_MAX_ITEMS) == 25);
    // T0 item: high 4 / low 8; T1 item: high 8 / low 4; level 1 phir 0.
    CHECK(items[0].duration0 == 4 && items[0].level0 == 1 &&
          items[0].duration1 == 8 && items[0].level1 == 0);  // G bit7 = 0
    CHECK(items[2].duration0 == 8 && items[2].duration1 == 4);  // G bit5 = 1
    CHECK(items[6].duration0 == 8 && items[6].duration1 == 4);  // G bit1 = 1
    CHECK(items[7].duration0 == 4 && items[7].duration1 == 8);  // G bit0 = 0
    CHECK(items[11].duration0 == 8 && items[11].duration1 == 4);  // R bit4 = 1
    CHECK(items[15].duration0 == 8 && items[15].duration1 == 4);  // R bit0 = 1
    CHECK(items[18].duration0 == 8 && items[18].duration1 == 4);  // B bit5 = 1
    CHECK(items[23].duration0 == 8 && items[23].duration1 == 4);  // B bit0 = 1
    // Aakhri item reset: 50 us LOW.
    CHECK(items[24].duration0 == 500 && items[24].level0 == 0 &&
          items[24].duration1 == 0 && items[24].level1 == 0);
    // --- All-zero pixel: 24 x T0 + reset; all-255: 24 x T1 + reset.
    uint8_t px0[3] = {0, 0, 0};
    CHECK(rsh_ws2812_encode(px0, 1, items, RSH_WS2812_MAX_ITEMS) == 25);
    for (int i = 0; i < 24; i++)
      CHECK(items[i].duration0 == 4 && items[i].duration1 == 8);
    uint8_t pxf[3] = {255, 255, 255};
    CHECK(rsh_ws2812_encode(pxf, 1, items, RSH_WS2812_MAX_ITEMS) == 25);
    for (int i = 0; i < 24; i++)
      CHECK(items[i].duration0 == 8 && items[i].duration1 == 4);
    // --- Driver -> encode chain: SOLID red, 16 LEDs, driver frame ko
    // encode karo — 385 items, pehle 8 G=0 (T0), agle 8 R=255 (T1),
    // aakhri 8 B=0 (T0), phir reset item.
    RshLedDriver sd;
    CHECK(rsh_led_driver_init(&sd, 16, led_sink_rec, NULL) == 1);
    RshLedPattern sred = {RSH_LED_SOLID, 255, 0, 0, 1000, 0, 0};
    CHECK(rsh_led_driver_start(&sd, &sred, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&sd, 0) == 1);
    CHECK(led_sink_writes == 1 && led_sink_counts[0] == 16);
    CHECK(rsh_ws2812_encode((const uint8_t *)led_sink_frames[0], 16, items,
                            RSH_WS2812_MAX_ITEMS) == 385);
    for (int i = 0; i < 8; i++)
      CHECK(items[i].duration0 == 4);   // G = 0 -> T0
    for (int i = 8; i < 16; i++)
      CHECK(items[i].duration0 == 8);   // R = 255 -> T1
    for (int i = 16; i < 24; i++)
      CHECK(items[i].duration0 == 4);   // B = 0 -> T0
    CHECK(items[384].duration0 == 500 && items[384].level0 == 0);
    // --- SEC-H-10 (W7, Low): total run time cap 60 s (clamp).
    // repeats 255 x period 5000 ms ~21 min tha — ab 60 s par kata hai.
    RshLedPatternState cst;
    RshLedPattern cap = {RSH_LED_BLINK, 0, 255, 0, 5000, 255, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 60000);
    CHECK(rsh_led_pattern_done(&cst, 59999) == 0);
    CHECK(rsh_led_pattern_done(&cst, 60000) == 1);
    uint8_t cr, cg, cb;
    rsh_led_pattern_pixel(&cst, 60000, 0, 4, &cr, &cg, &cb);
    CHECK(cr == 0 && cg == 0 && cb == 0);  // cap ke baad all-off
    // SOLID repeats 255 (1 s/repeat) bhi 60 s par kata hai.
    cap = {RSH_LED_SOLID, 255, 0, 0, 0, 255, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 60000);
    CHECK(rsh_led_pattern_done(&cst, 59999) == 0);
    CHECK(rsh_led_pattern_done(&cst, 60000) == 1);
    // Boundary: repeats 60 x 1000 ms = exactly 60000 — cap nahi lagta,
    // phir bhi 60000 par done (barabar hai, zyada nahi).
    cap = {RSH_LED_BLINK, 0, 255, 0, 1000, 60, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 60000);
    CHECK(rsh_led_pattern_done(&cst, 59999) == 0);
    CHECK(rsh_led_pattern_done(&cst, 60000) == 1);
    // repeats 61 x 1000 = 61000 -> 60000 par cap.
    cap = {RSH_LED_BLINK, 0, 255, 0, 1000, 61, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 60000);
    // Cap se neeche wale repeats waisi ki waisi (repeats 2 x 1000 = 2000).
    cap = {RSH_LED_BLINK, 0, 255, 0, 1000, 2, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 2000);
    CHECK(rsh_led_pattern_done(&cst, 1999) == 0);
    CHECK(rsh_led_pattern_done(&cst, 2000) == 1);
    // repeats 0 -> 5 s default, cap se door.
    cap = {RSH_LED_RAINBOW, 0, 0, 0, 1000, 0, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 5000);
    // CHASE 5000 ms x 255 bhi cap hota hai; OFF total 0 rehta hai.
    cap = {RSH_LED_CHASE, 0, 0, 255, 5000, 255, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 60000);
    cap = {RSH_LED_OFF, 0, 0, 0, 0, 0, 0};
    rsh_led_pattern_start(&cst, &cap, 0);
    CHECK(cst.totalMs == 0);
    // Cap driver se bhi guzarta hai: BLINK 5000/255 start karo — driver
    // 60 s par all-off karta hai (tick 59999 par active, 60000 par done).
    RshLedDriver cd;
    CHECK(rsh_led_driver_init(&cd, 4, led_sink_rec, NULL) == 1);
    RshLedPattern cblink = {RSH_LED_BLINK, 0, 255, 0, 5000, 255, 0};
    CHECK(rsh_led_driver_start(&cd, &cblink, 0, 21, 0, 1) == RSH_CTRL_OK);
    led_sink_writes = 0;
    CHECK(rsh_led_driver_tick(&cd, 0) == 1);  // on (phase 0..2500)
    CHECK(rsh_led_driver_active(&cd) == 1);
    CHECK(rsh_led_driver_tick(&cd, 2500) == 1);   // off (phase 2500..5000)
    CHECK(rsh_led_driver_tick(&cd, 52500) == 0);  // off hi — no write
    CHECK(rsh_led_driver_tick(&cd, 55000) == 1);  // on dobara (phase 0)
    CHECK(rsh_led_driver_active(&cd) == 1);       // 60 s se pehle active
    CHECK(rsh_led_driver_tick(&cd, 60000) == 1);  // cap -> all-off ek dafa
    CHECK(rsh_led_driver_active(&cd) == 0);
    CHECK(led_sink_frames[led_sink_writes - 1][0].g == 0);
  }

  // 24) AS5600 magnetic encoder (Phase G WS4, as5600.h / D34).
  {
    // Raw -> mV boundaries (D6: sab axes mV, 0..3300).
    CHECK(rsh_as5600_raw_to_mv(0) == 0);
    CHECK(rsh_as5600_raw_to_mv(4095) == 3300);
    CHECK(rsh_as5600_raw_to_mv(2048) == 1650);
    CHECK(rsh_as5600_raw_to_mv(1024) == 825);
    CHECK(rsh_as5600_raw_to_mv(5000) == 3300);  // clamp
    // STATUS decode (0x0B: MD bit5, ML bit4, MH bit3).
    RshAs5600Status st = rsh_as5600_status_decode(0x20);
    CHECK(st.magnet_detected == 1 && st.too_weak == 0 && st.too_strong == 0);
    st = rsh_as5600_status_decode(0x10);
    CHECK(st.magnet_detected == 0 && st.too_weak == 1 && st.too_strong == 0);
    st = rsh_as5600_status_decode(0x08);
    CHECK(st.magnet_detected == 0 && st.too_weak == 0 && st.too_strong == 1);
    st = rsh_as5600_status_decode(0x38);
    CHECK(st.magnet_detected == 1 && st.too_weak == 1 && st.too_strong == 1);
    // AGC band @3.3 V (range 0..128, ideal ~64).
    CHECK(rsh_as5600_agc_ok(64) == 1);
    CHECK(rsh_as5600_agc_ok(16) == 1 && rsh_as5600_agc_ok(112) == 1);
    CHECK(rsh_as5600_agc_ok(0) == 0 && rsh_as5600_agc_ok(128) == 0);
    // Turn tracking: wrap-aware delta.
    RshAs5600Turns tr;
    rsh_as5600_turns_init(&tr);
    CHECK(rsh_as5600_turn_update(&tr, 4000) == 0);  // pehla sample: sirf state
    CHECK(rsh_as5600_turn_update(&tr, 5) == 101);   // 4000 -> 5 wrap: +101
    CHECK(tr.total_steps == 101);
    CHECK(rsh_as5600_turn_update(&tr, 4090) == -11);  // 5 -> 4090 wrap: -11
    CHECK(tr.total_steps == 90);
    // 3 turns aage: 48 ticks x 256 steps = 12288 steps = 3 x 4096.
    RshAs5600Turns big;
    rsh_as5600_turns_init(&big);
    rsh_as5600_turn_update(&big, 0);
    for (int i = 0; i < 3 * 16; i++)
      rsh_as5600_turn_update(&big, (uint16_t)(((i + 1) * 256) % 4096));
    CHECK(big.total_steps == 12288);
    CHECK(rsh_as5600_steps_to_mv(big.total_steps, RSH_AS5600_ROT_900_STEPS)
          == 3300);  // clamp: 12288 > 10240
    // 900° mapping: 450° = 5120 steps -> 1650 mV.
    CHECK(rsh_as5600_steps_to_mv(5120, RSH_AS5600_ROT_900_STEPS) == 1650);
    CHECK(rsh_as5600_steps_to_mv(0, RSH_AS5600_ROT_900_STEPS) == 0);
    CHECK(rsh_as5600_steps_to_mv(10240, RSH_AS5600_ROT_900_STEPS) == 3300);
    CHECK(rsh_as5600_steps_to_mv(-5, RSH_AS5600_ROT_900_STEPS) == 0);
    CHECK(rsh_as5600_steps_to_mv(2048, RSH_AS5600_ROT_360_STEPS) == 1650);
    // Glitch: ek tick mein >90° (1024 steps) jump reject hota hai.
    RshAs5600Turns g;
    rsh_as5600_turns_init(&g);
    rsh_as5600_turn_update(&g, 100);
    CHECK(rsh_as5600_turn_update(&g, 2000) == 0);  // +1900 steps = glitch
    CHECK(g.total_steps == 0 && g.glitches == 1);
    CHECK(rsh_as5600_turn_update(&g, 2100) == 100);  // resync ke baad normal
    // Mock I2C read path (STATUS + RAWANGLE + AGC).
    as5600_mock_raw = 2048;
    as5600_mock_agc = 64;
    as5600_mock_status = 0x20;
    as5600_mock_fail = 0;
    RshAs5600Sample smp;
    CHECK(rsh_as5600_read(as5600_mock_read, NULL, &smp) == 0);
    CHECK(smp.valid == 1 && smp.raw == 2048 && smp.agc == 64);
    CHECK(rsh_as5600_raw_to_mv(smp.raw) == 1650);
    CHECK(rsh_as5600_probe(as5600_mock_read, NULL) == 1);
    // Magnet ghayab: MD=0 -> valid=0 (axis-invalid rasta, F2/D12).
    as5600_mock_status = 0x00;
    CHECK(rsh_as5600_read(as5600_mock_read, NULL, &smp) == 0);
    CHECK(smp.valid == 0 && smp.magnet_ok == 0);
    // Too weak (ML=1) lekin magnet detected -> valid rehta hai (degraded).
    as5600_mock_status = 0x30;
    CHECK(rsh_as5600_read(as5600_mock_read, NULL, &smp) == 0);
    CHECK(smp.valid == 1);
    // I2C NACK: read + probe dono fail.
    as5600_mock_fail = 1;
    CHECK(rsh_as5600_read(as5600_mock_read, NULL, &smp) != 0);
    CHECK(smp.valid == 0);
    CHECK(rsh_as5600_probe(as5600_mock_read, NULL) == 0);
    as5600_mock_fail = 0;
    // Config blob type 12 (config_transaction.h): AS5600 record validate
    // + axis count; doosra AS5600 reject; ghalat rotation reject.
    uint8_t recs[32];
    uint8_t blob[256];
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_AS5600, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
    size_t blen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, blen, NULL, NULL) == RSH_CTRL_OK);
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_AS5600, 0, 0, 0xFF, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recs + 16, RSH_CFG_TYPE_AXIS_AS5600, 1, 0, 0xFF, 0xFF, 0xFF, NULL);
    blen = rsh_cfg_blob_build(blob, recs, 2, 1);
    CHECK(rsh_cfg_blob_validate(blob, blen, NULL, NULL) == RSH_CTRL_PIN_CONFLICT);
    uint8_t badrot[10] = {0, 3, 0, 0, 0, 0, 0, 0, 0, 0};  // params[1] = 3
    rsh_cfg_record(recs, RSH_CFG_TYPE_AXIS_AS5600, 0, 0, 0xFF, 0xFF, 0xFF, badrot);
    blen = rsh_cfg_blob_build(blob, recs, 1, 1);
    CHECK(rsh_cfg_blob_validate(blob, blen, NULL, NULL) == RSH_CTRL_PIN_CONFLICT);
  }

  // ---- §22: wired USB-serial framing (Phase G WS1, D26 / OPEN-20) ----
  {
    uint8_t frame[RSH_SERIAL_MAX_FRAME];
    uint8_t payload[RSH_SERIAL_MAX_PAYLOAD + 16];
    uint8_t out[RSH_SERIAL_MAX_PAYLOAD + 16];
    uint8_t kind = 0;
    uint16_t plen = 0;

    // Canonical vectors (Python + C# se byte-exact match hona chahiye):
    // V1: kind=Stream(1), payload 01..08 -> ...d95d (CRC 0x5DD9).
    uint8_t v1p[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    size_t flen = rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_STREAM, v1p, 8);
    CHECK(flen == 17);
    uint8_t v1exp[17] = {0x52, 0x53, 0x48, 0x53, 0x01, 0x08, 0x00,
                         0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0xd9, 0x5d};
    CHECK(memcmp(frame, v1exp, 17) == 0);
    // V2: kind=Helo(6), payload "HELO" 02 00 -> ...4749 (CRC 0x4947).
    uint8_t v2p[6] = {'H', 'E', 'L', 'O', 2, 0};
    flen = rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_HELO, v2p, 6);
    CHECK(flen == 15);
    uint8_t v2exp[15] = {0x52, 0x53, 0x48, 0x53, 0x06, 0x06, 0x00,
                         0x48, 0x45, 0x4c, 0x4f, 0x02, 0x00, 0x47, 0x49};
    CHECK(memcmp(frame, v2exp, 15) == 0);

    // Round-trip har kind ke liye (payload alag-alag length).
    for (uint8_t k = RSH_SERIAL_KIND_STREAM; k <= RSH_SERIAL_KIND_PROBE; k++) {
      uint16_t pl = (uint16_t)(k * 7 + 3);  // 10..52
      for (uint16_t i = 0; i < pl; i++) payload[i] = (uint8_t)(i * 11 + k);
      flen = rsh_serial_build_frame(frame, sizeof(frame), k, payload, pl);
      CHECK(flen == (size_t)RSH_SERIAL_HEADER_SIZE + pl + RSH_SERIAL_CRC_SIZE);
      // CRC independent table-driven impl se bhi match kare.
      CHECK(rsh_crc16_ccitt(frame, flen - 2) == crc_table_impl(frame, flen - 2));
      CHECK(rsh_serial_parse_frame(frame, flen, &kind, out, &plen) == RSH_SERIAL_OK);
      CHECK(kind == k && plen == pl && memcmp(out, payload, pl) == 0);
    }

    // Empty payload (PROBE, len 0) OK.
    flen = rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_PROBE, NULL, 0);
    CHECK(flen == RSH_SERIAL_HEADER_SIZE + RSH_SERIAL_CRC_SIZE);
    CHECK(rsh_serial_parse_frame(frame, flen, &kind, out, &plen) == RSH_SERIAL_OK);
    CHECK(kind == RSH_SERIAL_KIND_PROBE && plen == 0);

    // Max payload 1053 OK; 1054 reject; kind 0/8 reject; buf chhota reject.
    memset(payload, 0xAB, sizeof(payload));
    flen = rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_CONTROL, payload, 1053);
    CHECK(flen == RSH_SERIAL_MAX_FRAME);
    CHECK(rsh_serial_parse_frame(frame, flen, &kind, out, &plen) == RSH_SERIAL_OK);
    CHECK(plen == 1053 && memcmp(out, payload, 1053) == 0);
    CHECK(rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_CONTROL, payload, 1054) == 0);
    CHECK(rsh_serial_build_frame(frame, sizeof(frame), 0, payload, 1) == 0);
    CHECK(rsh_serial_build_frame(frame, sizeof(frame), 8, payload, 1) == 0);
    CHECK(rsh_serial_build_frame(frame, 10, RSH_SERIAL_KIND_STREAM, payload, 8) == 0);

    // Parse errors: TOO_SHORT sirf <7 B par; us ke baad BAD_LENGTH.
    uint8_t small[6] = {'R', 'S', 'H', 'S', 1, 8};
    CHECK(rsh_serial_parse_frame(small, 6, &kind, out, &plen) == RSH_SERIAL_TOO_SHORT);
    CHECK(rsh_serial_parse_frame(small, 0, &kind, out, &plen) == RSH_SERIAL_TOO_SHORT);
    uint8_t badmagic[17];
    memcpy(badmagic, v1exp, 17); badmagic[0] = 'X';
    CHECK(rsh_serial_parse_frame(badmagic, 17, &kind, out, &plen) == RSH_SERIAL_BAD_MAGIC);
    uint8_t badcrc[17];
    memcpy(badcrc, v1exp, 17); badcrc[16] ^= 0xFF;
    CHECK(rsh_serial_parse_frame(badcrc, 17, &kind, out, &plen) == RSH_SERIAL_BAD_CRC);
    // Truncated frame (header poora, payload adhoora) -> BAD_LENGTH (B1 rule).
    CHECK(rsh_serial_parse_frame(v1exp, 12, &kind, out, &plen) == RSH_SERIAL_BAD_LENGTH);
    // Len field 1054 -> BAD_LENGTH; kind 9 -> BAD_LENGTH; extra byte -> BAD_LENGTH.
    uint8_t badlen[17];
    memcpy(badlen, v1exp, 17); badlen[5] = 0x1E; badlen[6] = 0x04;  // len 1054
    CHECK(rsh_serial_parse_frame(badlen, 17, &kind, out, &plen) == RSH_SERIAL_BAD_LENGTH);
    memcpy(badlen, v1exp, 17); badlen[4] = 9;
    CHECK(rsh_serial_parse_frame(badlen, 17, &kind, out, &plen) == RSH_SERIAL_BAD_LENGTH);
    uint8_t extra[18];
    memcpy(extra, v1exp, 17); extra[17] = 0;
    CHECK(rsh_serial_parse_frame(extra, 18, &kind, out, &plen) == RSH_SERIAL_BAD_LENGTH);

    // Decoder: byte-at-a-time, phir chunked, phir garbage resync.
    rsh_serial_decoder_t dec;
    rsh_serial_decoder_init(&dec);
    flen = rsh_serial_build_frame(frame, sizeof(frame), RSH_SERIAL_KIND_STREAM, v1p, 8);
    uint32_t got = 0;
    for (size_t i = 0; i < flen; i++)
      if (rsh_serial_decoder_feed(&dec, frame[i])) {
        got++;
        CHECK(dec.frame_kind == RSH_SERIAL_KIND_STREAM && dec.frame_len == 8);
        CHECK(memcmp(dec.buf + RSH_SERIAL_HEADER_SIZE, v1p, 8) == 0);
      }
    CHECK(got == 1 && dec.frames_ok == 1);
    // Chunked: aadha frame, phir baqi — ek hi frame milna chahiye.
    rsh_serial_decoder_init(&dec);
    CHECK(rsh_serial_decoder_feed_buf(&dec, frame, 9) == 0);
    CHECK(rsh_serial_decoder_feed_buf(&dec, frame + 9, flen - 9) == 1);
    CHECK(dec.frames_ok == 1 && dec.frame_kind == RSH_SERIAL_KIND_STREAM);
    // Garbage (boot log) ke baad valid frame: resync, bad_magic gina jaye.
    rsh_serial_decoder_init(&dec);
    uint8_t garbage[6] = {'b', 'o', 'o', 't', '\r', '\n'};
    CHECK(rsh_serial_decoder_feed_buf(&dec, garbage, 6) == 0);
    CHECK(dec.bad_magic > 0);
    CHECK(rsh_serial_decoder_feed_buf(&dec, frame, flen) == 1);
    CHECK(dec.frames_ok == 1);
    // Bad CRC wala frame gira, counter barhe, agla valid frame phir bhi aaye.
    rsh_serial_decoder_init(&dec);
    CHECK(rsh_serial_decoder_feed_buf(&dec, badcrc, 17) == 0);
    CHECK(dec.bad_crc == 1 && dec.frames_ok == 0);
    CHECK(rsh_serial_decoder_feed_buf(&dec, frame, flen) == 1);
    CHECK(dec.frames_ok == 1 && dec.bad_crc == 1);
    // Do frame peeche peeche (feed_buf sirf aakhri chhorta hai — ginti 2).
    rsh_serial_decoder_init(&dec);
    uint8_t two[34];
    memcpy(two, frame, flen); memcpy(two + flen, frame, flen);
    CHECK(rsh_serial_decoder_feed_buf(&dec, two, 2 * flen) == 2);
    CHECK(dec.frames_ok == 2);
    // Oversize len header (kind 4, len 1054) -> bad_length, resync.
    rsh_serial_decoder_init(&dec);
    uint8_t oshdr[7] = {'R', 'S', 'H', 'S', 4, 0x1E, 0x04};
    CHECK(rsh_serial_decoder_feed_buf(&dec, oshdr, 7) == 0);
    CHECK(dec.bad_length == 1);
    CHECK(rsh_serial_decoder_feed_buf(&dec, frame, flen) == 1);
    CHECK(dec.frames_ok == 1);
  }

  printf("%s — %d checks, %d failures\n", failures ? "TESTS FAILED" : "ALL TESTS PASSED", checks, failures);
  return failures ? 1 : 0;
}
