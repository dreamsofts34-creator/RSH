#include "inputs.h"
#include "protocol.h"
#include <Arduino.h>

static Debouncer dbGearUp, dbGearDown, dbAccel, dbBrake;
static bool useMv = (STEER_USE_MILLIVOLTS != 0);
static SampleGovernor governor;   // .ino window ke hisaab se update karta hai

bool inputs_steering_is_mv() { return useMv; }
SampleGovernor &inputs_governor() { return governor; }

static uint16_t read_steering_once() {
  return useMv ? (uint16_t)analogReadMilliVolts(PIN_STEER)
               : (uint16_t)analogRead(PIN_STEER);
}

static uint16_t avg_steering() {
  (void)read_steering_once();   // channel switch ke baad pehli conversion
                                // pichle channel ki taraf bias hoti hai —
                                // dummy read discard karo (audit fix)
  uint8_t n = governor.steerSamples();
  if (n > STEER_SAMPLES) n = STEER_SAMPLES;   // buffer kabhi overflow na ho
                                              // (governor aur config ka
                                              // ittefaq mustaqbil mein bhi)
  uint16_t v[STEER_SAMPLES];
  for (uint8_t i = 0; i < n; i++) v[i] = read_steering_once();
  return apex_trimmed_mean(v, n, n >= 12 ? 2 : 1);   // spikes trim ho jati hain
}

static uint16_t avg_pedal(uint8_t pin) {
  (void)analogRead(pin);        // dummy read — channel-switch bias discard
  uint8_t n = governor.pedalSamples();
  if (n < 1) n = 1;                       // divide-by-zero ka koi rasta na ho
  if (n > PEDAL_SAMPLES) n = PEDAL_SAMPLES;
  uint32_t sum = 0;
  for (uint8_t i = 0; i < n; i++) sum += (uint32_t)analogRead(pin);
  return (uint16_t)(sum / n);
}

void inputs_begin() {
  analogReadResolution(12);              // board ka native size
  analogSetAttenuation(ADC_11db);        // default bhi yehi hai; wazeh likh diya
  pinMode(PIN_ACCEL_BTN, INPUT_PULLUP);
  pinMode(PIN_BRAKE_BTN, INPUT_PULLUP);
  pinMode(PIN_GEAR_UP, INPUT_PULLUP);
  pinMode(PIN_GEAR_DOWN, INPUT_PULLUP);

  // mV mode ka startup cross-check (audit fix): calibration fail ho to
  // analogReadMilliVolts khamoshi se 0 deta hai. Raw reading bari ho aur
  // mV 0 aye to raw mode par fallback — packet flags asal mode batayenge.
  if (useMv) {
    uint32_t rawSum = 0, mvSum = 0;
    for (int i = 0; i < 8; i++) {
      rawSum += (uint32_t)analogRead(PIN_STEER);
      mvSum += (uint32_t)analogReadMilliVolts(PIN_STEER);
    }
    if (mvSum == 0 && rawSum > 8 * 200) useMv = false;
  }
}

InputSample inputs_read() {
  InputSample s;
  s.steering = avg_steering();
  s.accelAnalog = avg_pedal(PIN_ACCEL_ADC);
  s.brakeAnalog = avg_pedal(PIN_BRAKE_ADC);

  // Buttons active-low hain (INPUT_PULLUP, doosri taraf GND)
  uint8_t b = 0;
  if (dbGearUp.update(digitalRead(PIN_GEAR_UP) == LOW))     b |= APEX_BTN_GEAR_UP;
  if (dbGearDown.update(digitalRead(PIN_GEAR_DOWN) == LOW)) b |= APEX_BTN_GEAR_DOWN;
  if (dbAccel.update(digitalRead(PIN_ACCEL_BTN) == LOW))    b |= APEX_BTN_ACCEL;
  if (dbBrake.update(digitalRead(PIN_BRAKE_BTN) == LOW))    b |= APEX_BTN_BRAKE;
  s.buttons = b;
  return s;
}
