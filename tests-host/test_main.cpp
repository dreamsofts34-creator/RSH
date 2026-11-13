// RSH host-side virtual tests — protocol + debounce + scheduler logic.
// Ye wohi header files istemal karte hain jo firmware compile karta hai.
#include <cstdio>
#include <cstring>
#include <cstdint>
#include <cstdlib>
#include "protocol.h"
#include "inputs.h"

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

static uint32_t rng_state = 0x12345678;
static uint32_t rng() { rng_state = rng_state * 1664525u + 1013904223u; return rng_state >> 8; }

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

  // 5) Debouncer: 2 lagataar samples par hi state badle; bounce reject ho
  {
    Debouncer d;   // needed = DEBOUNCE_SAMPLES = 2
    CHECK(d.update(false) == false);
    CHECK(d.update(true) == false);    // sirf 1 sample — abhi nahi
    CHECK(d.update(false) == false);   // bounce — streak reset
    CHECK(d.update(true) == false);
    CHECK(d.update(true) == true);     // 2 lagataar — ab pressed
    CHECK(d.update(true) == true);
    CHECK(d.update(false) == true);    // release par bhi 2 chahiye
    CHECK(d.update(false) == false);
    // alternating bounce kabhi state na badle
    Debouncer e;
    for (int i = 0; i < 20; i++) e.update(i % 2 == 0);
    CHECK(e.state == false);
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

  // 9) Adaptive sampling governor: dabao par step down, sakoon par step up
  {
    SampleGovernor g;
    CHECK(g.level == 0 && g.steerSamples() == 16 && g.pedalSamples() == 8);
    g.update(3);                       // zyada overruns — neeche
    CHECK(g.level == 1 && g.steerSamples() == 12 && g.pedalSamples() == 6);
    g.update(10);
    CHECK(g.level == 2 && g.steerSamples() == 8 && g.pedalSamples() == 4);
    g.update(10);                      // is se neeche nahi
    CHECK(g.level == 2);
    for (int i = 0; i < 4; i++) g.update(0);
    CHECK(g.level == 2);               // 4 saaf windows kaafi nahi
    g.update(0);                       // 5th saaf window — ek step upar
    CHECK(g.level == 1);
    g.update(1);                       // 1-2 overruns: level waisa hi
    CHECK(g.level == 1);
    g.update(2);
    CHECK(g.level == 1);
    for (int i = 0; i < 5; i++) g.update(0);
    CHECK(g.level == 0);
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

  printf("%s — %d checks, %d failures\n", failures ? "TESTS FAILED" : "ALL TESTS PASSED", checks, failures);
  return failures ? 1 : 0;
}
