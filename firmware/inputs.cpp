#include "inputs.h"
#include "protocol.h"
#include <Arduino.h>
#include <Wire.h>
#include "as5600.h"
#include "ads1115.h"
#include "i2c_axes.h"

static Debouncer dbGearUp, dbGearDown, dbAccel, dbBrake, dbHorn;  // FW-11: horn
static Debouncer dbHandbrake;  // F-15 (v4.0.2): handbrake button mode
static uint16_t lastHandbrakeDiag = 0;   // FW-11: handbrake trimmed-mean state
static uint16_t lastClutchDiag = 0;      // Phase F WS5: clutch trimmed-mean state
// D-4 (v4.0.2): shifter sensitivity (0..100, default 50) — sequential
// gear debounce isi se chalta hai.
// v2.0.19: ye statics W2 mein ghalti se H-pattern block ke saath delete
// ho gaye thay (sequential debounce in par chalta hai — inputs_apply_
// config_blob + debouncer reset inhein istemal karte hain); wapas lao.
static uint8_t shifterSensitivity = RSH_CFG_SHIFTER_SENSITIVITY_DEFAULT;
static uint8_t shifterDebounceMs =
    13;  // rsh_shifter_debounce_ms(50) = 20 - 50*15/100 = 13 (formula se)
static bool useMv = (STEER_USE_MILLIVOLTS != 0);
static SampleGovernor governor;   // .ino window ke hisaab se update karta hai
static uint32_t lastMvCheckMs = 0;
static bool accelAnalogMode = true, brakeAnalogMode = true;
static uint32_t pedalCycle = 0;   // button-mode diagnostic cadence ke liye
static uint16_t lastAccelDiag = 0, lastBrakeDiag = 0;
static AxisHealth healthSteer, healthAccel, healthBrake, healthClutch;
static AxisHealth healthHandbrake;  // F-04: handbrake health (slot 4)
static uint8_t axisInvalidMask = 0;
// Movement tracking — rail-stuck faisle ke liye "doosre axes move kar rahe
// hon" (D12): har axis ki aakhri harkat ka waqt. (Yahan upar hain taake
// inputs_apply_config_blob() inhein reset kar sake — wo neeche hai.)
static uint16_t prevSteer = 0, prevAccel = 0, prevBrake = 0, prevClutch = 0;
static uint16_t prevHandbrake = 0;  // F-04
static bool hasPrev = false;
static uint32_t movedAtSteer = 0, movedAtAccel = 0, movedAtBrake = 0, movedAtClutch = 0;
static uint32_t movedAtHandbrake = 0;  // F-04
// Phase G (C3): live pin table — compiled defaults, phir config blob se.
// inputs_read har jagah PIN_* ki jagah is table se parhta hai; blob
// badle to inputs_apply_config_blob() table badal kar pinMode dobara karta hai.
static RshPinMap pinMap;
static bool pinMapReady = false;
static bool forceMvProbe = false;  // steering pin badle to agle read par mV probe
// FW-01 (Round 3): I2C axes sampler + Wire bus state. Sampler pure hai
// (`i2c_axes.h` — host tests mein mock I2C se covered); yahan sirf Wire
// callbacks + bus management hain.
static RshI2cAxes i2cAxes;
static bool i2cBusStarted = false;
// F-01 (polish round): I2C poll backoff — lagatar fail hone par bus ko
// har cycle hammer karne ke bajaye 500 ms skip karo, aur backoff lagte
// hi ek dafa bus recovery (9 SCL pulses + STOP) chalao. Ek mara hua
// device warna har 5 ms cycle mein Wire timeout khata (sample task
// stall -> 5 s task WDT -> reboot loop).
static uint8_t i2cFailStreak = 0;
static uint32_t i2cBackoffUntilMs = 0;
static bool i2cRecoveredThisBackoff = false;
// F-16 (polish round): mV probe ab stepped hai — 8 raw+mV pairs 8 cycles
// mein (2 reads/cycle), ek cycle mein 16 reads nahi. Probe state:
static bool mvProbeActive = false;
static uint8_t mvProbeCount = 0;
static uint32_t mvProbeRawSum = 0, mvProbeMvSum = 0;

// Wire callbacks (as5600.h / ads1115.h signature) — register likho, phir
// bytes parho (repeated start, endTransmission(false)).
static int i2c_wire_read_(uint8_t dev, uint8_t reg, uint8_t *buf, uint8_t len,
                          void *user) {
  (void)user;
  Wire.beginTransmission(dev);
  Wire.write(reg);
  if (Wire.endTransmission(false) != 0) return -1;  // NACK / bus error
  if (Wire.requestFrom((int)dev, (int)len) != len) return -1;
  for (uint8_t i = 0; i < len; i++) {
    if (!Wire.available()) return -1;
    buf[i] = (uint8_t)Wire.read();
  }
  return 0;
}
static int i2c_wire_write_(uint8_t dev, uint8_t reg, const uint8_t *buf,
                           uint8_t len, void *user) {
  (void)user;
  Wire.beginTransmission(dev);
  Wire.write(reg);
  for (uint8_t i = 0; i < len; i++) Wire.write(buf[i]);
  return Wire.endTransmission() == 0 ? 0 : -1;
}

// F-01: SDA/SCL bus recovery — Wire band karo, SCL par 9 clock pulses
// (stuck slave apna byte poora karke SDA chhore), phir STOP condition,
// phir Wire dobara start. Sirf backoff lagne par ek dafa chalti hai;
// NACK karne wala device (sahi bus, mara hua chip) par bhi be-zarar hai.
static void i2c_bus_recover_(void) {
  Wire.end();
  pinMode(21, OUTPUT);
  pinMode(22, OUTPUT);
  digitalWrite(21, HIGH);
  for (int i = 0; i < 9; i++) {
    digitalWrite(22, LOW);
    delayMicroseconds(5);
    digitalWrite(22, HIGH);
    delayMicroseconds(5);
  }
  digitalWrite(21, LOW);   // STOP: SDA LOW jab SCL HIGH
  delayMicroseconds(5);
  digitalWrite(22, LOW);
  delayMicroseconds(5);
  digitalWrite(22, HIGH);
  delayMicroseconds(5);
  digitalWrite(21, HIGH);
  delayMicroseconds(5);
  pinMode(21, INPUT_PULLUP);
  pinMode(22, INPUT_PULLUP);
  Wire.begin(21, 22);
  Wire.setClock(400000);
  Wire.setTimeout(5);  // F-01: requestFrom/endTransmission 5 ms se zyada
                       // block na kare (Arduino default timeout kabhi
                       // set hi nahi hua tha)
  i2cBusStarted = true;
}

// I2C bus sirf tab start karo jab koi I2C kind configure ho — warna
// GPIO21 LED strip ka pin rehta hai (validator R-L9: I2C kinds hon to
// 21/22 I2C hain). Bus band bhi karo jab I2C khatam ho (LED wapas).
// Sampler config har map change par dobara lagti hai (bus pehle se on
// ho to bhi — channel/rotation badal sakta hai).
static void i2c_bus_ensure_(void) {
  bool need = rsh_pinmap_has_i2c(&pinMap);
  if (need && !i2cBusStarted) {
    Wire.begin(21, 22);     // SDA GPIO21, SCL GPIO22 (validator R-L9)
    Wire.setClock(400000);  // Fast mode — AS5600 + ADS1115 dono support
                            // karte hain; sample cycle (5 ms) mein I2C
                            // cost ~0.25 ms rehta hai (100 kHz par ~1 ms)
    Wire.setTimeout(5);     // F-01: I2C transaction 5 ms se zyada block
                            // na kare (sample task + 5 s task WDT)
    i2cBusStarted = true;
    i2cFailStreak = 0;
    i2cBackoffUntilMs = 0;
    i2cRecoveredThisBackoff = false;
  } else if (!need && i2cBusStarted) {
    Wire.end();
    i2cBusStarted = false;
  }
  if (need) rsh_i2c_axes_configure(&i2cAxes, &pinMap);
  else rsh_i2c_axes_init(&i2cAxes);
}

static void pinmap_ensure_defaults_(void) {
  if (!pinMapReady) {
    rsh_pinmap_defaults(&pinMap);
    pinMapReady = true;
  }
}

// Live table ke digital pins par pinMode karo. Purane pins jo ab istemal
// nahi hote, un ka INPUT_PULLUP rehta hai (Arduino mein un-configure
// karne ka tareeqa nahi — pull-up ke saath floating input mehfooz hai,
// aur unhein parha bhi nahi jata). Analog pins par pinMode NAHI hota.
static void pinmap_apply_pinmodes_(void) {
  pinMode(PIN_ACCEL_BTN, INPUT_PULLUP);   // fixed pedal button-mode pins
  pinMode(PIN_BRAKE_BTN, INPUT_PULLUP);
  if (pinMap.gearUp != RSH_PINMAP_ABSENT) pinMode(pinMap.gearUp, INPUT_PULLUP);
  if (pinMap.gearDown != RSH_PINMAP_ABSENT) pinMode(pinMap.gearDown, INPUT_PULLUP);
  if (pinMap.horn != RSH_PINMAP_ABSENT) pinMode(pinMap.horn, INPUT_PULLUP);
}

uint8_t inputs_apply_config_blob(const uint8_t *blob, size_t len) {
  RshPinMap next;
  if (!rsh_pinmap_from_blob(blob, len, &next)) return 0;  // invalid — kuch nahi badla
  // D-4: sensitivity blob se aati hai (pin map waisa hi ho to bhi badal
  // sakti hai — is liye memcmp early-return se PEHLE update karo).
  shifterSensitivity = rsh_cfg_shifter_sensitivity(blob, len);
  shifterDebounceMs = rsh_shifter_debounce_ms(shifterSensitivity);
  dbGearUp.debounceMs = shifterDebounceMs;
  dbGearDown.debounceMs = shifterDebounceMs;
  // F-09 (polish round): handbrake debounce FIXED 10 ms (DEBOUNCE_MS,
  // Debouncer default) — shifter sensitivity se nahi chalta. Pehle
  // sensitivity 100 par handbrake 5 ms debounce leta tha (bounce-prone
  // switch flicker karta tha) aur 0 par 20 ms (sluggish).
  dbHandbrake.debounceMs = DEBOUNCE_MS;
  pinmap_ensure_defaults_();
  if (memcmp(&next, &pinMap, sizeof(next)) == 0) return 1;  // wahi map — re-init nahi
  pinMap = next;
  pinmap_apply_pinmodes_();
  i2c_bus_ensure_();  // FW-01: I2C axis aaye/gaye to Wire start/stop + sampler config
  // HID P1-2 (polish round): pedal button-mode switch. Axis record ho
  // (pin assigned ya I2C axis) to analog; record hi na ho to button mode
  // — fixed pins 25/26 debounced axis 4095/0 chalate hain (neeche
  // inputs_read mein). Pehle inputs_set_pedal_analog ka koi caller nahi
  // tha, is liye button-mode pedals ka axis stale/floating analog par
  // rehta tha aur bits 2/3 wheel buttons 3/4 ban kar leak hote thay.
  inputs_set_pedal_analog(
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_ACCEL),
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_BRAKE));
  // Naye pins = nayi state: purane pin ki debouncer/health state
  // naye pin par ghalat trigger na kare (e.g. purana gear-up daba hua
  // tha, naya pin HIGH hai — release ka intezar kiye baghair saaf karo).
  dbGearUp = Debouncer{};
  dbGearDown = Debouncer{};
  dbAccel = Debouncer{};
  dbBrake = Debouncer{};
  dbHorn = Debouncer{};
  dbHandbrake = Debouncer{};
  dbGearUp.debounceMs = shifterDebounceMs;
  dbGearDown.debounceMs = shifterDebounceMs;
  dbHandbrake.debounceMs = DEBOUNCE_MS;  // F-09: fixed 10 ms (upar dekho)
  healthSteer = AxisHealth{};
  healthAccel = AxisHealth{};
  healthBrake = AxisHealth{};
  healthClutch = AxisHealth{};
  healthHandbrake = AxisHealth{};  // F-04
  prevSteer = prevAccel = prevBrake = prevClutch = prevHandbrake = 0;
  hasPrev = false;
  movedAtSteer = movedAtAccel = movedAtBrake = movedAtClutch = movedAtHandbrake = 0;
  axisInvalidMask = 0;
  lastHandbrakeDiag = 0;
  lastClutchDiag = 0;
  lastAccelDiag = 0;
  lastBrakeDiag = 0;
  forceMvProbe = true;  // mV probe agle read par naye steering pin par
  return 1;
}

const RshPinMap *inputs_pin_map() {
  pinmap_ensure_defaults_();
  return &pinMap;
}

bool inputs_steering_is_mv() {
  // FW-01: I2C steering ki value hamesha mV hoti hai (AS5600/ADS1115 mV
  // mein convert karte hain) — native ADC par useMv probe ka faisla chalta hai.
  pinmap_ensure_defaults_();
  if (pinMap.axisType[RSH_PINMAP_AXIS_STEERING] ==
          RSH_CFG_TYPE_AXIS_AS5600 ||
      pinMap.axisType[RSH_PINMAP_AXIS_STEERING] ==
          RSH_CFG_TYPE_AXIS_ADS1115)
    return true;
  return useMv;
}

// P1-1: har slot ke liye mV flag (stream_emit axes_is_mv mask).
// I2C axis (ADS1115/AS5600) ki value sampler se mV mein aati hai;
// native steering mV mode mein bhi mV hai (useMv probe); baaqi native
// axes raw counts hain. Button-mode handbrake (4095/0 raw) mV NAHI hai —
// us par raw->mV convert hona chahiye (4095 -> 3300 full), is liye
// axisType check hi kaafi hai (button mode ka type AXIS_ADC hai).
bool inputs_axis_is_mv(uint8_t slot) {
  pinmap_ensure_defaults_();
  if (slot > RSH_PINMAP_AXIS_HANDBRAKE) return false;
  uint8_t t = pinMap.axisType[slot];
  if (t == RSH_CFG_TYPE_AXIS_AS5600 || t == RSH_CFG_TYPE_AXIS_ADS1115)
    return true;
  if (slot == RSH_PINMAP_AXIS_STEERING) return useMv;
  return false;
}
SampleGovernor &inputs_governor() { return governor; }
uint8_t inputs_axis_invalid_mask() { return axisInvalidMask; }
bool inputs_axis_invalid() { return axisInvalidMask != 0; }

void inputs_set_pedal_analog(bool accelAnalog, bool brakeAnalog) {
  accelAnalogMode = accelAnalog;
  brakeAnalogMode = brakeAnalog;
}

static uint16_t read_steering_once() {
  if (pinMap.steer == RSH_PINMAP_ABSENT) return 0;  // steering unassigned
  return useMv ? (uint16_t)analogReadMilliVolts(pinMap.steer)
               : (uint16_t)analogRead(pinMap.steer);
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
  return rsh_trimmed_mean(v, n, n >= 12 ? 2 : 1);   // spikes trim ho jati hain
}

// Pedal: trimmed mean (F6 — plain mean khatam, ek spike pedal ko nahi hilata).
// Button mode mein analog skip (F7/D14): sirf har 20th cycle diagnostic read.
static uint16_t avg_pedal(uint8_t pin, bool analogMode, uint16_t &lastDiag) {
  if (!rsh_pedal_should_read(analogMode, pedalCycle)) return lastDiag;
  (void)analogRead(pin);        // dummy read — channel-switch bias discard
  uint8_t n = governor.pedalSamples();
  if (n < 1) n = 1;                       // divide-by-zero ka koi rasta na ho
  if (n > PEDAL_SAMPLES) n = PEDAL_SAMPLES;
  uint16_t v[PEDAL_SAMPLES];
  for (uint8_t i = 0; i < n; i++) v[i] = (uint16_t)analogRead(pin);
  lastDiag = rsh_trimmed_mean(v, n, n >= 4 ? 1 : 0);
  return lastDiag;
}

// mV mode ka cross-check: 8 raw + 8 mV samples se faisla (boot + periodic
// recheck dono yahi helper istemal karte hain — F13). Returns: true = mV
// theek hai, false = raw par fallback karo. Steering pin live table se
// aata hai (Phase G, C3); unassigned ho to koi probe nahi (useMv waisa hi).
static bool mv_probe_ok() {
  if (pinMap.steer == RSH_PINMAP_ABSENT) return useMv;
  uint32_t rawSum = 0, mvSum = 0;
  for (int i = 0; i < 8; i++) {
    rawSum += (uint32_t)analogRead(pinMap.steer);
    mvSum += (uint32_t)analogReadMilliVolts(pinMap.steer);
  }
  if (useMv) return !rsh_mv_should_fallback(rawSum, mvSum, 8);
  return rsh_mv_should_restore(mvSum, 8);
}

// F-16: stepped mV probe — mv_probe_ok() ka ek pair (1 raw + 1 mV)
// per cycle. inputs_read har cycle ek pair chalata hai; 8 pairs ke baad
// faisla wahi hota hai jo mv_probe_ok() deta (boot par blocking probe
// hi rehta hai — wahan cycle budget nahi hai).
// Return: -1 = probe chal rahi, 0 = complete (useMv waisa hi),
// 1 = complete (useMv badla).
static int mv_probe_step_() {
  if (!mvProbeActive) {
    mvProbeActive = true;
    mvProbeCount = 0;
    mvProbeRawSum = 0;
    mvProbeMvSum = 0;
  }
  if (pinMap.steer == RSH_PINMAP_ABSENT) {
    mvProbeActive = false;
    return 0;
  }
  mvProbeRawSum += (uint32_t)analogRead(pinMap.steer);
  mvProbeMvSum += (uint32_t)analogReadMilliVolts(pinMap.steer);
  if (++mvProbeCount < 8) return -1;
  mvProbeActive = false;
  bool ok = useMv ? !rsh_mv_should_fallback(mvProbeRawSum, mvProbeMvSum, 8)
                  : rsh_mv_should_restore(mvProbeMvSum, 8);
  if (ok == useMv) return 0;
  useMv = !useMv;
  return 1;
}

void inputs_begin() {
  analogReadResolution(12);              // board ka native size
  analogSetAttenuation(ADC_11db);        // default bhi yehi hai; wazeh likh diya
  pinmap_ensure_defaults_();   // pehle compiled defaults, phir .ino blob lagata hai
  // FW-11: horn button.
  // Phase G (C3): pinMode ab live pin table se hota hai (pinmap_apply_
  // pinmodes_ — defaults yahan, blob ke pins COMMIT par dobara).
  pinmap_apply_pinmodes_();
  // FW-01: I2C sampler init + bus (defaults mein I2C nahi — sirf state
  // saaf hoti hai; blob apply par i2c_bus_ensure_ dobara chalta hai).
  rsh_i2c_axes_init(&i2cAxes);
  i2c_bus_ensure_();

  // mV mode ka startup cross-check (audit fix): calibration fail ho to
  // analogReadMilliVolts khamoshi se 0 deta hai. Raw reading bari ho aur
  // mV 0 aye to raw mode par fallback — packet flags asal mode batayenge.
  if (useMv && !mv_probe_ok()) useMv = false;
  lastMvCheckMs = millis();
}

// FW-01: ek axis slot ki value — I2C axis ho to sampler se (mV), native
// ho to ADC trimmed-mean se, absent ho to 0. Steering is se nahi parhta
// (us ka apna 16-sample avg_steering path hai).
static uint16_t read_axis_value_(uint8_t slot, uint8_t pin, bool analogMode,
                                 uint16_t &lastDiag) {
  if (pin == RSH_PINMAP_ABSENT) {
    if (rsh_pinmap_axis_present(&pinMap, slot))
      return i2cAxes.value_mv[slot];  // I2C axis (AS5600/ADS1115), mV
    return 0;
  }
  return avg_pedal(pin, analogMode, lastDiag);
}

// FW-01: axis ki rail (health check) — I2C value mV hoti hai (3300),
// native ADC raw hoti hai (4095). Steering ke liye inputs_steering_is_mv()
// istemal hota hai (native steering mV mode mein bhi ho sakta hai).
static uint16_t axis_rail_(uint8_t slot, uint8_t pin) {
  if (pin == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, slot))
    return 3300;
  return 4095;
}

InputSample inputs_read() {
  uint32_t nowMs = millis();

  // FW-01: I2C axes poll karo (AS5600 har cycle, ADS1115 20 ms cadence,
  // cached values). Bus on ho to hi — configure ke baghair poll no-op hai.
  // F-01: lagatar 5 fail -> 500 ms backoff (bus hammer nahi) + ek dafa
  // bus recovery. Backoff ke dauraan valid[] waisa hi rehta hai (axis
  // invalid flag fail-closed rehta hai), values stale rehti hain.
  if (i2cBusStarted) {
    if (i2cBackoffUntilMs != 0 &&
        (uint32_t)(nowMs - i2cBackoffUntilMs) < 0x80000000u) {
      i2cBackoffUntilMs = 0;  // backoff khatam — dobara poll karo
      i2cRecoveredThisBackoff = false;
    }
    if (i2cBackoffUntilMs == 0) {
      int rc = rsh_i2c_axes_poll(&i2cAxes, i2c_wire_read_, i2c_wire_write_,
                                 NULL, nowMs);
      if (rc == 0) {
        i2cFailStreak = 0;
        i2cRecoveredThisBackoff = false;
      } else if (++i2cFailStreak >= 5) {
        i2cBackoffUntilMs = nowMs + 500;
        i2cFailStreak = 0;
        if (!i2cRecoveredThisBackoff) {
          i2c_bus_recover_();
          i2cRecoveredThisBackoff = true;
        }
      }
    }
  }

  // mV fallback periodic recheck (F13 — sirf boot par nahi): raw mode mein
  // har 30 s dobara probe karo; calibration wapas aaye to mV restore karo.
  // mV mode mein bhi probe karo — calibration beech mein mare to fallback.
  // Phase G (C3): steering pin badle to forceMvProbe agle read par probe
  // karwata hai (nay pin ki calibration alag ho sakti hai).
  // F-16: probe stepped hai — har cycle sirf 1 raw + 1 mV read (8 cycles
  // mein poora), ek cycle mein 16 reads ka spike nahi.
  if (pinMap.steer != RSH_PINMAP_ABSENT) {
    if (mvProbeActive) {
      (void)mv_probe_step_();
    } else if (forceMvProbe ||
               (uint32_t)(nowMs - lastMvCheckMs) >= RSH_MV_RECHECK_MS) {
      lastMvCheckMs = nowMs;
      forceMvProbe = false;
      (void)mv_probe_step_();  // pehla pair isi cycle
    }
  }

  InputSample s{};   // value-init: band module ki values 0 rehti hain
                     // (clutch/handbrake sirf enabled hone par bhari
                     // jati hain — F7; enc hamesha 0 — v2.0.19),
                     // garbage kabhi nahi jata
  // FW-01: I2C steering (AS5600/ADS1115) ki value sampler se aati hai
  // (mV); native steering trimmed-mean ADC path se; unassigned 0.
  if (pinMap.steer == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_STEERING))
    s.steering = i2cAxes.value_mv[RSH_PINMAP_AXIS_STEERING];
  else
    s.steering = avg_steering();
  // HID P1-2: pedal button debouncers EK dafa yahan update karo — axis
  // drive (button mode) isi state se aata hai. Dobara update karna
  // debounce counting bigarta (ek cycle mein 2 observations).
  // Round 2 W3 guard: pin 25/26 kisi aur function ko diye hon to
  // button hi nahi (axis 0 rehta hai). v2.0.19: pedal button BITS
  // (2/3) khatam — debounced state sirf axis chalati hai, button
  // word mein nahi jati (neeche dekho).
  bool accelBtnPressed = false, brakeBtnPressed = false;
  if (!rsh_pinmap_uses_pin(&pinMap, PIN_ACCEL_BTN))
    accelBtnPressed =
        dbAccel.update(digitalRead(PIN_ACCEL_BTN) == LOW, nowMs);
  if (!rsh_pinmap_uses_pin(&pinMap, PIN_BRAKE_BTN))
    brakeBtnPressed =
        dbBrake.update(digitalRead(PIN_BRAKE_BTN) == LOW, nowMs);
  // Phase G (C3): axes live pin table se; unassigned axis (record nahi)
  // sample nahi hota — value 0 (avg_pedal kabhi 0xFF pin par nahi bulaya jata).
  // FW-01: I2C axis ho to read_axis_value_ sampler se deta hai.
  // HID P1-2: button mode mein axis debounced button se chalta hai
  // (4095 daba hua / 0 chhora hua) — pehle stale/floating analog jata tha.
  if (accelAnalogMode) {
    s.accelAnalog = read_axis_value_(RSH_PINMAP_AXIS_ACCEL, pinMap.accel,
                                     accelAnalogMode, lastAccelDiag);
  } else {
    s.accelAnalog = accelBtnPressed ? 4095 : 0;
  }
  if (brakeAnalogMode) {
    s.brakeAnalog = read_axis_value_(RSH_PINMAP_AXIS_BRAKE, pinMap.brake,
                                     brakeAnalogMode, lastBrakeDiag);
  } else {
    s.brakeAnalog = brakeBtnPressed ? 4095 : 0;
  }
  // FW-11 (F7): band module ka ADC read NAHI hota — handbrake band ho to
  // avg_pedal call hi nahi hota (koi ADC conversion, koi dummy read nahi),
  // value 0 rehti hai aur axis mask bit bhi clear hoti hai (modules.h).
  // F-15 (v4.0.2): button mode mein pin DIGITAL button hai — daba hua =
  // axis 4095 (full), chhora hua = 0 (debounced, sensitivity ke mutabiq).
  if (rsh_modules_handbrake_enabled() &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_HANDBRAKE)) {
    if (pinMap.handbrakeButtonMode) {
      s.handbrake =
          dbHandbrake.update(digitalRead(pinMap.handbrake) == LOW, nowMs)
              ? 4095 : 0;
    } else if (pinMap.handbrake == RSH_PINMAP_ABSENT) {
      // FW-01: I2C handbrake (ADS1115/AS5600) — sampler value (mV).
      s.handbrake = i2cAxes.value_mv[RSH_PINMAP_AXIS_HANDBRAKE];
    } else {
      s.handbrake = avg_pedal(pinMap.handbrake, true, lastHandbrakeDiag);
    }
  } else {
    s.handbrake = 0;
  }
  // Phase F WS5 (F7): clutch band ho to ADC read nahi hota (value 0,
  // axis mask bit bhi clear — modules.h). Clutch pin GPIO33 default
  // map mein gear-down bhi hai — clutch on ho to gear-down ka digital
  // read ADC read se takrata nahi (alag peripheral), lekin config
  // validator dono ko ek saath allow nahi karta (PIN_CONFLICT).
  // Phase G (C3): clutch pin blob se aata hai (kisi bhi ADC1 pin par).
  // FW-01: I2C clutch bhi — read_axis_value_ sampler se deta hai.
  s.clutch = rsh_modules_clutch_enabled()
      ? read_axis_value_(RSH_PINMAP_AXIS_CLUTCH, pinMap.clutch, true,
                         lastClutchDiag)
      : 0;
  pedalCycle++;

  // v2.0.19: encoders DELETE — wire slots barqarar hain (protocol v2.2
  // frozen), values hamesha 0. Koi quadrature read nahi hota.
  s.enc[0] = 0;
  s.enc[1] = 0;
  s.enc[2] = 0;
  s.enc[3] = 0;

  // Floating/disconnected detection (F2/D12): rail-stuck + doosre axes
  // move, ya high jitter. Still wheel (variance≈0) kabhi invalid nahi.
  if (hasPrev) {
    if (s.steering > prevSteer + 30 || prevSteer > s.steering + 30)
      movedAtSteer = nowMs;
    if (s.accelAnalog > prevAccel + 30 || prevAccel > s.accelAnalog + 30)
      movedAtAccel = nowMs;
    if (s.brakeAnalog > prevBrake + 30 || prevBrake > s.brakeAnalog + 30)
      movedAtBrake = nowMs;
    if (s.clutch > prevClutch + 30 || prevClutch > s.clutch + 30)
      movedAtClutch = nowMs;
    if (s.handbrake > prevHandbrake + 30 || prevHandbrake > s.handbrake + 30)
      movedAtHandbrake = nowMs;  // F-04
  }
  prevSteer = s.steering; prevAccel = s.accelAnalog; prevBrake = s.brakeAnalog;
  prevClutch = s.clutch;
  prevHandbrake = s.handbrake;  // F-04
  hasPrev = true;
  bool othersForSteer = (uint32_t)(nowMs - movedAtAccel) < 2000 ||
                        (uint32_t)(nowMs - movedAtBrake) < 2000 ||
                        (uint32_t)(nowMs - movedAtClutch) < 2000;
  bool othersForAccel = (uint32_t)(nowMs - movedAtSteer) < 2000 ||
                        (uint32_t)(nowMs - movedAtBrake) < 2000 ||
                        (uint32_t)(nowMs - movedAtClutch) < 2000;
  bool othersForBrake = (uint32_t)(nowMs - movedAtSteer) < 2000 ||
                        (uint32_t)(nowMs - movedAtAccel) < 2000 ||
                        (uint32_t)(nowMs - movedAtClutch) < 2000;
  bool othersForClutch = (uint32_t)(nowMs - movedAtSteer) < 2000 ||
                         (uint32_t)(nowMs - movedAtAccel) < 2000 ||
                         (uint32_t)(nowMs - movedAtBrake) < 2000;
  bool othersForHandbrake = (uint32_t)(nowMs - movedAtSteer) < 2000 ||  // F-04
                            (uint32_t)(nowMs - movedAtAccel) < 2000 ||
                            (uint32_t)(nowMs - movedAtBrake) < 2000 ||
                            (uint32_t)(nowMs - movedAtClutch) < 2000;
  // mV mode mein steering rail ~3300 mV hai, raw mode mein 4095.
  // FW-01: I2C steering bhi mV hai (inputs_steering_is_mv).
  uint16_t steerRail = inputs_steering_is_mv() ? 3300 : 4095;
  axisInvalidMask = 0;
  // Phase G (C3): unassigned axis ka health check nahi hota — absent
  // axis ki value 0 hoti hai, jo rail-stuck jaisi dikhti hai; us par
  // invalid bit lagana ghalat hota (axis maujood hi nahi).
  // FW-01: I2C axis ka bhi health check hota hai (axis_present — pin
  // ya I2C kind); rail mV ke hisaab se (axis_rail_).
  // F-03: I2C axis ka rail test nahi chalta (values rail par EXACT
  // clamp hoti hain) aur button-mode axis ka bhi nahi (4095/0 binary hai
  // — daba hua pedal/handbrake rail hi hota hai). Dono ke liye sirf
  // jitter rule aur sampler valid[] flags chalte hain.
  auto axis_is_i2c = [](uint8_t slot) {
    uint8_t t = pinMap.axisType[slot];
    return t == RSH_CFG_TYPE_AXIS_AS5600 || t == RSH_CFG_TYPE_AXIS_ADS1115;
  };
  if (rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_STEERING) &&
      healthSteer.update(s.steering, steerRail, othersForSteer, nowMs,
                         !axis_is_i2c(RSH_PINMAP_AXIS_STEERING)))
    axisInvalidMask |= 0x01;
  if (rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_ACCEL) &&
      healthAccel.update(s.accelAnalog,
                         axis_rail_(RSH_PINMAP_AXIS_ACCEL, pinMap.accel),
                         othersForAccel, nowMs,
                         accelAnalogMode &&
                             !axis_is_i2c(RSH_PINMAP_AXIS_ACCEL)))
    axisInvalidMask |= 0x02;
  if (rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_BRAKE) &&
      healthBrake.update(s.brakeAnalog,
                         axis_rail_(RSH_PINMAP_AXIS_BRAKE, pinMap.brake),
                         othersForBrake, nowMs,
                         brakeAnalogMode &&
                             !axis_is_i2c(RSH_PINMAP_AXIS_BRAKE)))
    axisInvalidMask |= 0x04;
  // Phase F WS5: clutch health bit 3. Band module ka clutch hamesha 0
  // hai — AxisHealth rail-stuck is par bhi chalega, lekin band module
  // ki bit ko .ino axis mask se hata deta hai (mask mein clutch bit
  // sirf enabled hone par set hoti hai), is liye mask mein ghalat
  // invalid bit nahi jati. Phase G (C3): unassigned clutch ka bhi
  // health check nahi hota (upar wali wajah).
  if (rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_CLUTCH) &&
      healthClutch.update(s.clutch,
                          axis_rail_(RSH_PINMAP_AXIS_CLUTCH, pinMap.clutch),
                          othersForClutch, nowMs,
                          !axis_is_i2c(RSH_PINMAP_AXIS_CLUTCH)))
    axisInvalidMask |= 0x08;
  // F-04: handbrake health (slot 4) — pehle koi coverage nahi thi.
  // Button mode mein rail test nahi (4095/0 binary); I2C par bhi nahi.
  if (rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_HANDBRAKE) &&
      healthHandbrake.update(s.handbrake,
                             axis_rail_(RSH_PINMAP_AXIS_HANDBRAKE,
                                        pinMap.handbrake),
                             othersForHandbrake, nowMs,
                             !pinMap.handbrakeButtonMode &&
                                 !axis_is_i2c(RSH_PINMAP_AXIS_HANDBRAKE)))
    axisInvalidMask |= 0x10;
  // FW-01: I2C axis fail (NACK / magnet ghayab) -> invalid bit (last
  // value rehti hai — health check alag se rail-stuck bhi pakad sakta
  // hai, lekin ye bit seedha sampler ki validity se aata hai).
  if (pinMap.steer == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_STEERING) &&
      !i2cAxes.valid[RSH_PINMAP_AXIS_STEERING])
    axisInvalidMask |= 0x01;
  if (pinMap.accel == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_ACCEL) &&
      !i2cAxes.valid[RSH_PINMAP_AXIS_ACCEL])
    axisInvalidMask |= 0x02;
  if (pinMap.brake == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_BRAKE) &&
      !i2cAxes.valid[RSH_PINMAP_AXIS_BRAKE])
    axisInvalidMask |= 0x04;
  if (pinMap.clutch == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_CLUTCH) &&
      !i2cAxes.valid[RSH_PINMAP_AXIS_CLUTCH])
    axisInvalidMask |= 0x08;
  // F-04: I2C handbrake fail (NACK / magnet ghayab) -> invalid bit 4.
  if (pinMap.handbrake == RSH_PINMAP_ABSENT &&
      rsh_pinmap_axis_present(&pinMap, RSH_PINMAP_AXIS_HANDBRAKE) &&
      !i2cAxes.valid[RSH_PINMAP_AXIS_HANDBRAKE])
    axisInvalidMask |= 0x10;

  // Buttons active-low hain (INPUT_PULLUP, doosri taraf GND).
  // Debounce time-based hai (F12) — governor level se aazad, hamesha 10 ms.
  // FW-11: band module ka button bit NAHI jata — shifter band ho to
  // gear up/down bits clear, horn band ho to horn bit clear (read bhi
  // nahi hota, F7 ki tarah).
  // v2.0.19: button word mein SIRF bits 0/1/4 (shifter up/down, horn)
  // aate hain — accel/brake pedal button bits (2/3) DELETE hue, wire
  // par hamesha 0. accelBtnPressed/brakeBtnPressed sirf pedal AXIS
  // button-mode chalate hain (upar), button word mein nahi jate.
  uint8_t b = 0;
  if (rsh_modules_shifter_enabled()) {
    // Phase G (C3): gear pins live table se; unassigned button ka
    // debouncer update nahi hota (bit kabhi nahi aata).
    if (pinMap.gearUp != RSH_PINMAP_ABSENT &&
        dbGearUp.update(digitalRead(pinMap.gearUp) == LOW, nowMs))   b |= RSH_BTN_GEAR_UP;
    if (pinMap.gearDown != RSH_PINMAP_ABSENT &&
        dbGearDown.update(digitalRead(pinMap.gearDown) == LOW, nowMs)) b |= RSH_BTN_GEAR_DOWN;
  }
  if (rsh_modules_horn_enabled() && pinMap.horn != RSH_PINMAP_ABSENT &&
      dbHorn.update(digitalRead(pinMap.horn) == LOW, nowMs))    b |= RSH_BTN_HORN;
  s.buttons = b;
  return s;
}
