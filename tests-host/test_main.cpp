// ApexLink host-side virtual tests — protocol + debounce + scheduler logic.
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

static uint32_t rng_state = 0x12345678;
static uint32_t rng() { rng_state = rng_state * 1664525u + 1013904223u; return rng_state >> 8; }

int main() {
  crc_table_init();

  // 1) CRC standard check value (RevEng catalogue: CRC-16/CCITT-FALSE)
  const char *vec = "123456789";
  CHECK(apex_crc16_ccitt((const uint8_t *)vec, 9) == 0x29B1);
  CHECK(crc_table_impl((const uint8_t *)vec, 9) == 0x29B1);
  // khaali data par init value hi rehti hai
  CHECK(apex_crc16_ccitt((const uint8_t *)vec, 0) == 0xFFFF);

  // 2) Dono CRC implementations 500 random buffers par mutafiq hon
  for (int t = 0; t < 500; t++) {
    uint8_t buf[64];
    size_t n = rng() % 65;
    for (size_t i = 0; i < n; i++) buf[i] = (uint8_t)rng();
    CHECK(apex_crc16_ccitt(buf, n) == crc_table_impl(buf, n));
  }

  // 3) Packet golden bytes — layout bilkul docs (02) ke mutabiq ho
  uint8_t pkt[APEX_PACKET_SIZE];
  size_t len = apex_build_packet(pkt, 0x1234, 0x01020304, 2300, 1000, 2000, 0x05, 0x01);
  CHECK(len == 18);
  const uint8_t expect[16] = {0xA5, 0x01, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01,
                              0xFC, 0x08, 0xE8, 0x03, 0xD0, 0x07, 0x05, 0x01};
  CHECK(memcmp(pkt, expect, 16) == 0);
  uint16_t crc = crc_table_impl(pkt, 16);   // independent implementation se
  CHECK(pkt[16] == (crc & 0xFF) && pkt[17] == (crc >> 8));

  // 4) Packet ke har field ki boundary values (0 aur max)
  apex_build_packet(pkt, 0xFFFF, 0xFFFFFFFFu, 4095, 0, 3300, 0xFF, 0xFF);
  CHECK(pkt[2] == 0xFF && pkt[3] == 0xFF);
  CHECK(pkt[8] == 0xFF && pkt[9] == 0x0F);          // 4095 = 0x0FFF
  CHECK(pkt[12] == 0xE4 && pkt[13] == 0x0C);        // 3300 = 0x0CE4
  CHECK(apex_crc16_ccitt(pkt, 16) == (uint16_t)(pkt[16] | (pkt[17] << 8)));

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
    CHECK(apex_trimmed_mean(steady, 16, 2) == 2000);

    uint16_t spiked[16];
    for (int i = 0; i < 16; i++) spiked[i] = 2000;
    spiked[3] = 4095;    // ek ADC spike
    spiked[9] = 0;       // ek dropout
    CHECK(apex_trimmed_mean(spiked, 16, 2) == 2000);   // dono trim ho gaye

    uint16_t ramp[8] = {0, 100, 200, 300, 400, 500, 600, 700};
    // trim=1: 0 aur 700 hate, baqi 100..600 ka average = 350
    CHECK(apex_trimmed_mean(ramp, 8, 1) == 350);
    // trim=0: plain average = 350 bhi yahan wahi (symmetric), farq spike par ata hai
    uint16_t one[1] = {1234};
    CHECK(apex_trimmed_mean(one, 1, 0) == 1234);
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

  printf("%s — %d checks, %d failures\n", failures ? "TESTS FAILED" : "ALL TESTS PASSED", checks, failures);
  return failures ? 1 : 0;
}
