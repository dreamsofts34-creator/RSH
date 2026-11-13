// RSH firmware .cpp host tests (FW-13, T4 — sirf headers nahi).
// Asal inputs.cpp + status_led.cpp ko Arduino stub ke saath host par
// compile karke chalata hai (g++ -Wall -Wextra -Werror -fsanitize=...).
// inputs.cpp ke static state ki wajah se har mode alag process run hai:
//   ./run_tests_host_cpp mv-ok        — mV calibration theek (mV mode)
//   ./run_tests_host_cpp mv-fallback  — mV 0 + raw bari (raw fallback)
// Dono modes mein pedal/encoder/button/LED tests chalte hain.
#include <cstdio>
#include <cstring>
#include <cstdint>
#include <cstdlib>
#include "Arduino.h"

// ---- Fake Arduino state (stub definitions) ----
static uint32_t g_fake_millis = 0;
static uint16_t g_analog_const[64];
static uint32_t g_mv_const[64];
static uint8_t g_digital[64];
static uint8_t g_pin_written[64];
static uint8_t g_pinmode_mode[64];     // Phase G C3: pinMode recording
static uint32_t g_pinmode_count[64];   // (COMMIT par re-init verify karne ke liye)
static int g_write_count = 0;
static uint16_t g_analog_seq[64][80];
static int g_analog_seq_len[64];
static int g_analog_seq_pos[64];
static uint32_t g_mv_seq[64][80];
static int g_mv_seq_len[64];
static int g_mv_seq_pos[64];

void pinMode(uint8_t pin, uint8_t mode) {
  // Phase G C3: record karo — COMMIT par pinMode dobara hona chahiye.
  if (pin < 64) {
    g_pinmode_mode[pin] = mode;
    g_pinmode_count[pin]++;
  }
}
void digitalWrite(uint8_t pin, uint8_t value) {
  if (pin < 64) g_pin_written[pin] = value;
  g_write_count++;
}
int digitalRead(uint8_t pin) { return pin < 64 ? g_digital[pin] : HIGH; }
int analogRead(uint8_t pin) {
  if (pin >= 64) return 0;
  if (g_analog_seq_pos[pin] < g_analog_seq_len[pin])
    return g_analog_seq[pin][g_analog_seq_pos[pin]++];
  return g_analog_const[pin];
}
uint32_t analogReadMilliVolts(uint8_t pin) {
  if (pin >= 64) return 0;
  if (g_mv_seq_pos[pin] < g_mv_seq_len[pin])
    return g_mv_seq[pin][g_mv_seq_pos[pin]++];
  return g_mv_const[pin];
}
void analogReadResolution(uint8_t bits) { (void)bits; }
void analogSetAttenuation(int attenuation) { (void)attenuation; }
uint32_t millis() { return g_fake_millis; }
uint32_t micros() { return g_fake_millis * 1000u; }
void delay(uint32_t ms) { g_fake_millis += ms; }

// ---- Fake helpers ----
static void fake_set_analog_const(uint8_t pin, uint16_t v) {
  g_analog_const[pin] = v;
  g_analog_seq_len[pin] = 0;
  g_analog_seq_pos[pin] = 0;
}
static void fake_set_mv_const(uint8_t pin, uint32_t v) {
  g_mv_const[pin] = v;
  g_mv_seq_len[pin] = 0;
  g_mv_seq_pos[pin] = 0;
}
static void fake_set_analog_seq(uint8_t pin, const uint16_t *vals, int n) {
  for (int i = 0; i < n && i < 80; i++) g_analog_seq[pin][i] = vals[i];
  g_analog_seq_len[pin] = n;
  g_analog_seq_pos[pin] = 0;
}
static void fake_set_mv_seq(uint8_t pin, const uint32_t *vals, int n) {
  for (int i = 0; i < n && i < 80; i++) g_mv_seq[pin][i] = vals[i];
  g_mv_seq_len[pin] = n;
  g_mv_seq_pos[pin] = 0;
}
static void fake_set_digital(uint8_t pin, uint8_t level) {
  if (pin < 64) g_digital[pin] = level;
}

// Asal firmware .cpp files — stub Arduino ke saath host par compile hoti hain.
#include "inputs.cpp"
#include "status_led.cpp"

static int failures = 0, checks = 0;
#define CHECK(cond) do { checks++; if (!(cond)) { failures++; \
  printf("FAIL %s:%d  %s\n", __FILE__, __LINE__, #cond); } } while (0)

int main(int argc, char **argv) {
  const char *mode = argc > 1 ? argv[1] : "mv-ok";
  bool fallback_mode = strcmp(mode, "mv-fallback") == 0;
  printf("== host .cpp tests — mode %s ==\n", mode);

  for (int i = 0; i < 64; i++) g_digital[i] = HIGH;  // pull-up: dabaa hua nahi
  g_fake_millis = 0;

  // ---- inputs_begin: mV probe ----
  fake_set_analog_const(PIN_STEER, fallback_mode ? 3000 : 2000);
  fake_set_mv_const(PIN_STEER, fallback_mode ? 0 : 1650);
  fake_set_analog_const(PIN_ACCEL_ADC, 1000);
  fake_set_analog_const(PIN_BRAKE_ADC, 500);
  fake_set_analog_const(PIN_HANDBRAKE_ADC, 800);
  inputs_begin();
  if (fallback_mode) {
    CHECK(inputs_steering_is_mv() == false);  // raw bari + mV 0 -> raw fallback
  } else {
    CHECK(inputs_steering_is_mv() == true);   // mV theek -> mV mode
  }

  // ---- Steering trimmed mean, asal inputs_read se (spike + dropout) ----
  // avg_steering: 1 dummy + 16 samples, trim 2.
  if (fallback_mode) {
    const uint16_t seq[17] = {2000, 2000, 2000, 2000, 4095, 2000, 2000, 2000,
                              2000, 0, 2000, 2000, 2000, 2000, 2000, 2000, 2000};
    fake_set_analog_seq(PIN_STEER, seq, 17);
  } else {
    const uint32_t seq[17] = {1650, 1650, 1650, 1650, 3300, 1650, 1650, 1650,
                              1650, 0, 1650, 1650, 1650, 1650, 1650, 1650, 1650};
    fake_set_mv_seq(PIN_STEER, seq, 17);
  }
  // Pedal spike: dummy + 8 samples, trim 1 (ek 4095 spike).
  const uint16_t accel_seq[9] = {1000, 1000, 1000, 4095, 1000, 1000, 1000, 1000, 1000};
  fake_set_analog_seq(PIN_ACCEL_ADC, accel_seq, 9);
  g_fake_millis = 100;
  InputSample s = inputs_read();
  CHECK(s.steering == (fallback_mode ? 2000 : 1650));  // spike/dropout trim hue
  CHECK(s.accelAnalog == 1000);   // pedal spike trim hua (F6)
  CHECK(s.brakeAnalog == 500);
  CHECK(s.handbrake == 800);
  CHECK(s.buttons == 0);

  // ---- Buttons: time-based debounce, asal inputs_read se (F12) ----
  fake_set_digital(PIN_GEAR_UP, LOW);   // daba hua (active-low)
  g_fake_millis = 200;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) == 0);   // abhi pending (0 ms)
  g_fake_millis = 205;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) == 0);   // 5 ms — abhi nahi
  g_fake_millis = 210;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) != 0);   // 10 ms poore — pressed
  fake_set_digital(PIN_GEAR_UP, HIGH);         // chhora
  g_fake_millis = 215;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) != 0);   // release bhi 10 ms mangta hai
  g_fake_millis = 220;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) != 0);   // 5 ms — release abhi pending
  g_fake_millis = 225;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_GEAR_UP) == 0);   // 10 ms poore — released
  // Horn (FW-11): daba hua -> bit, band module -> bit nahi.
  fake_set_digital(PIN_HORN_BTN, LOW);
  g_fake_millis = 300;
  s = inputs_read();
  g_fake_millis = 310;
  s = inputs_read();
  CHECK((s.buttons & RSH_BTN_HORN) != 0);
  RshModuleEnable off = {0, 0, 0, 0, 0, 0, 0};
  rsh_modules_set_enabled(&off);
  g_fake_millis = 400;
  s = inputs_read();
  CHECK(s.handbrake == 0);          // band module ka ADC read nahi (F7)
  CHECK(s.enc[0] == 0 && s.enc[1] == 0);  // band encoders report 0
  CHECK((s.buttons & RSH_BTN_HORN) == 0);  // band horn ka bit nahi
  RshModuleEnable on = {1, 1, 1, 1, 1, 0, 0};  // Phase F WS5: clutch off, sequential
  rsh_modules_set_enabled(&on);

  // ---- Encoders: quadrature steps, asal inputs_read se (FW-12) ----
  // enc1 A/B: 00 -> 10 -> 11 -> 01 -> 00 = +4 counts.
  const uint8_t cyc[5][2] = {{0, 0}, {1, 0}, {1, 1}, {0, 1}, {0, 0}};
  for (int i = 0; i < 5; i++) {
    fake_set_digital(PIN_ENC1_A, cyc[i][0] ? HIGH : LOW);
    fake_set_digital(PIN_ENC1_B, cyc[i][1] ? HIGH : LOW);
    fake_set_digital(PIN_HORN_BTN, HIGH);  // horn chhora (debounce saaf)
    g_fake_millis = 500 + (uint32_t)(i * 20);
    s = inputs_read();
  }
  CHECK(s.enc[0] == 4);
  CHECK(s.enc[1] == 0);  // enc2 hilaa nahi (00 par ruka)

  // ---- Pin-map board-apply (Phase G, C3): blob -> live pins, asal inputs_read se ----
  // Blob A: steering 32, clutch 33, gear-up 25; horn/handbrake/enc2/LED absent.
  // Factory blob wapas = rollback (purani pins).
  {
    uint8_t recsA[7 * 16];
    uint8_t blobA[256];
    rsh_cfg_record(recsA + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 32, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 3, 0, 33, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 4 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 5 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 25, 0xFF, 0xFF, NULL);
    rsh_cfg_record(recsA + 6 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
    size_t alen = rsh_cfg_blob_build(blobA, recsA, 7, 1);
    CHECK(alen == 8 + 7 * 16 + 4);
    fake_set_mv_const(32, 1650);
    fake_set_analog_const(32, 2000);
    fake_set_mv_const(34, 2000);   // purana pin — alag value taake move nazar aaye
    fake_set_analog_const(34, 1111);
    fake_set_analog_const(33, 700);  // clutch
    fake_set_digital(25, LOW);     // naya gear-up daba hua
    fake_set_digital(4, LOW);      // horn pin — blob mein nahi (absent)
    fake_set_digital(27, HIGH);    // purana gear-up — ab parha nahi jata
    uint32_t pm25_before = g_pinmode_count[25];
    CHECK(inputs_apply_config_blob(blobA, alen) == 1);
    rsh_modules_apply_config_blob(blobA, alen);  // .ino dono bulata hai
    CHECK(inputs_pin_map()->steer == 32);
    CHECK(inputs_pin_map()->clutch == 33);
    CHECK(inputs_pin_map()->horn == 0xFF);
    CHECK(g_pinmode_count[25] > pm25_before);  // COMMIT par pinMode dobara
    g_fake_millis = 600;
    s = inputs_read();
    CHECK(s.steering == 1650);  // nayi pin 32 se (purani 34 par 2000 hota)
    CHECK(s.clutch == 700);     // clutch ab pin 33 se
    CHECK(s.handbrake == 0);    // absent — sample nahi hota
    CHECK(s.enc[1] == 0);       // enc2 absent
    CHECK((s.buttons & RSH_BTN_GEAR_UP) == 0);  // debounce pending
    g_fake_millis = 610;
    s = inputs_read();
    CHECK((s.buttons & RSH_BTN_GEAR_UP) != 0);  // gear-up ab pin 25 par
    CHECK((s.buttons & RSH_BTN_HORN) == 0);     // horn absent — pin 4 LOW phir bhi bit nahi
    // Wahi blob dobara: apply OK lekin pinMode dobara nahi (no-op).
    uint32_t pm25_after = g_pinmode_count[25];
    CHECK(inputs_apply_config_blob(blobA, alen) == 1);
    CHECK(g_pinmode_count[25] == pm25_after);
    // Kharab blob (CRC toda): reject, map waisa hi.
    uint8_t badA[256];
    memcpy(badA, blobA, alen);
    badA[alen - 1] ^= 0xFF;
    CHECK(inputs_apply_config_blob(badA, alen) == 0);
    CHECK(inputs_pin_map()->steer == 32);
    // Rollback (factory blob): steering wapas pin 34 par (2000), gear-up 27 par.
    uint8_t factoryB[256];
    size_t flenB = rsh_cfg_factory_blob(factoryB);
    uint32_t pm27_before = g_pinmode_count[27];
    CHECK(inputs_apply_config_blob(factoryB, flenB) == 1);
    rsh_modules_apply_config_blob(factoryB, flenB);
    CHECK(inputs_pin_map()->steer == 34);
    CHECK(g_pinmode_count[27] > pm27_before);
    fake_set_digital(25, HIGH);   // purana gear-up chhora
    fake_set_digital(27, LOW);    // factory gear-up daba hua
    fake_set_digital(4, HIGH);    // horn chhora
    g_fake_millis = 700;
    s = inputs_read();
    CHECK(s.steering == 2000);    // wapas pin 34 se
    CHECK(s.clutch == 0);         // factory mein clutch nahi
    g_fake_millis = 710;
    s = inputs_read();
    CHECK((s.buttons & RSH_BTN_GEAR_UP) != 0);  // gear-up wapas pin 27 par
  }

  // ---- Status LED: asal status_led.cpp, fake digitalWrite (FW-12/R-L9) ----
  led_begin();
  CHECK(g_pin_written[PIN_LED] == LOW);  // off = LOW (active-HIGH LED)
  led_set(LedState::PcConnected);
  led_update(0);
  CHECK(g_pin_written[PIN_LED] == HIGH);  // solid on
  led_set(LedState::ApUpWaitingPc);       // 2 Hz: 250 ms on / 250 ms off
  led_update(0);
  CHECK(g_pin_written[PIN_LED] == HIGH);
  led_update(250);
  CHECK(g_pin_written[PIN_LED] == LOW);
  led_update(500);
  CHECK(g_pin_written[PIN_LED] == HIGH);
  led_set(LedState::Error);               // 8 Hz: ~62 ms on / ~62 ms off
  led_update(0);
  CHECK(g_pin_written[PIN_LED] == HIGH);
  led_update(62);
  CHECK(g_pin_written[PIN_LED] == LOW);
  led_set(LedState::Updating);            // 5 Hz: 100 ms on / 100 ms off
  led_update(0);
  CHECK(g_pin_written[PIN_LED] == HIGH);
  led_update(100);
  CHECK(g_pin_written[PIN_LED] == LOW);

  printf("%s — %d checks, %d failures\n", failures ? "TESTS FAILED" : "ALL TESTS PASSED", checks, failures);
  return failures ? 1 : 0;
}
