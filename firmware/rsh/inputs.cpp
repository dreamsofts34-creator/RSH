#include "inputs.h"
#include "protocol.h"
#include <Arduino.h>

static Debouncer dbGearUp, dbGearDown, dbAccel, dbBrake, dbHorn;  // FW-11: horn
static RshEncoder enc1, enc2;   // FW-12: quadrature decoders (cumulative i16)
static uint16_t lastHandbrakeDiag = 0;   // FW-11: handbrake trimmed-mean state
static uint16_t lastClutchDiag = 0;      // Phase F WS5: clutch trimmed-mean state
static bool useMv = (STEER_USE_MILLIVOLTS != 0);
static SampleGovernor governor;   // .ino window ke hisaab se update karta hai
static uint32_t lastMvCheckMs = 0;
static bool accelAnalogMode = true, brakeAnalogMode = true;
static uint32_t pedalCycle = 0;   // button-mode diagnostic cadence ke liye
static uint16_t lastAccelDiag = 0, lastBrakeDiag = 0;
static AxisHealth healthSteer, healthAccel, healthBrake, healthClutch;
static uint8_t axisInvalidMask = 0;
// Movement tracking — rail-stuck faisle ke liye "doosre axes move kar rahe
// hon" (D12): har axis ki aakhri harkat ka waqt. (Yahan upar hain taake
// inputs_apply_config_blob() inhein reset kar sake — wo neeche hai.)
static uint16_t prevSteer = 0, prevAccel = 0, prevBrake = 0, prevClutch = 0;
static bool hasPrev = false;
static uint32_t movedAtSteer = 0, movedAtAccel = 0, movedAtBrake = 0, movedAtClutch = 0;
// Phase G (C3): live pin table — compiled defaults, phir config blob se.
// inputs_read har jagah PIN_* ki jagah is table se parhta hai; blob
// badle to inputs_apply_config_blob() table badal kar pinMode dobara karta hai.
static RshPinMap pinMap;
static bool pinMapReady = false;
static bool forceMvProbe = false;  // steering pin badle to agle read par mV probe

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
  for (int i = 0; i < 6; i++)
    if (pinMap.shiftH[i] != RSH_PINMAP_ABSENT) pinMode(pinMap.shiftH[i], INPUT_PULLUP);
  if (pinMap.enc1A != RSH_PINMAP_ABSENT) pinMode(pinMap.enc1A, INPUT_PULLUP);
  if (pinMap.enc1B != RSH_PINMAP_ABSENT) pinMode(pinMap.enc1B, INPUT_PULLUP);
  if (pinMap.enc2A != RSH_PINMAP_ABSENT) pinMode(pinMap.enc2A, INPUT_PULLUP);
  if (pinMap.enc2B != RSH_PINMAP_ABSENT) pinMode(pinMap.enc2B, INPUT_PULLUP);
}

uint8_t inputs_apply_config_blob(const uint8_t *blob, size_t len) {
  RshPinMap next;
  if (!rsh_pinmap_from_blob(blob, len, &next)) return 0;  // invalid — kuch nahi badla
  pinmap_ensure_defaults_();
  if (memcmp(&next, &pinMap, sizeof(next)) == 0) return 1;  // wahi map — re-init nahi
  pinMap = next;
  pinmap_apply_pinmodes_();
  // Naye pins = nayi state: purane pin ki debouncer/encoder/health state
  // naye pin par ghalat trigger na kare (e.g. purana gear-up daba hua
  // tha, naya pin HIGH hai — release ka intezar kiye baghair saaf karo).
  dbGearUp = Debouncer{};
  dbGearDown = Debouncer{};
  dbAccel = Debouncer{};
  dbBrake = Debouncer{};
  dbHorn = Debouncer{};
  rsh_encoder_init(&enc1);
  rsh_encoder_init(&enc2);
  healthSteer = AxisHealth{};
  healthAccel = AxisHealth{};
  healthBrake = AxisHealth{};
  healthClutch = AxisHealth{};
  prevSteer = prevAccel = prevBrake = prevClutch = 0;
  hasPrev = false;
  movedAtSteer = movedAtAccel = movedAtBrake = movedAtClutch = 0;
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

bool inputs_steering_is_mv() { return useMv; }
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

void inputs_begin() {
  analogReadResolution(12);              // board ka native size
  analogSetAttenuation(ADC_11db);        // default bhi yehi hai; wazeh likh diya
  pinmap_ensure_defaults_();   // pehle compiled defaults, phir .ino blob lagata hai
  // FW-11: horn button. FW-12: encoder A/B pins.
  // Phase F WS5: H-pattern pins + encoder pins HAMESHA pinMode hote
  // hain — gate mode runtime par badal sakta hai (SET_CFG blob), is
  // liye compile-time #if se sirf ek set configure karna ghalat hota.
  // H-pattern pins 16/17/19 encoder pins se overlap karte hain — dono
  // set INPUT_PULLUP hain, dobara pinMode karna be-zarar hai; inputs_read
  // mein runtime mode check tay karta hai kya parha jata hai.
  // Phase G (C3): pinMode ab live pin table se hota hai (pinmap_apply_
  // pinmodes_ — defaults yahan, blob ke pins COMMIT par dobara).
  pinmap_apply_pinmodes_();
  rsh_encoder_init(&enc1);
  rsh_encoder_init(&enc2);

  // mV mode ka startup cross-check (audit fix): calibration fail ho to
  // analogReadMilliVolts khamoshi se 0 deta hai. Raw reading bari ho aur
  // mV 0 aye to raw mode par fallback — packet flags asal mode batayenge.
  if (useMv && !mv_probe_ok()) useMv = false;
  lastMvCheckMs = millis();
}

InputSample inputs_read() {
  uint32_t nowMs = millis();

  // mV fallback periodic recheck (F13 — sirf boot par nahi): raw mode mein
  // har 30 s dobara probe karo; calibration wapas aaye to mV restore karo.
  // mV mode mein bhi probe karo — calibration beech mein mare to fallback.
  // Phase G (C3): steering pin badle to forceMvProbe agle read par probe
  // karwata hai (nay pin ki calibration alag ho sakti hai).
  if (pinMap.steer != RSH_PINMAP_ABSENT &&
      (forceMvProbe || (uint32_t)(nowMs - lastMvCheckMs) >= RSH_MV_RECHECK_MS)) {
    lastMvCheckMs = nowMs;
    forceMvProbe = false;
    if (mv_probe_ok() != useMv) useMv = !useMv;
  }

  InputSample s{};   // value-init: band module ki values 0 rehti hain
                     // (clutch/handbrake/enc/gear sirf enabled hone par
                     // bhari jati hain — F7), garbage kabhi nahi jata
  s.steering = avg_steering();
  // Phase G (C3): axes live pin table se; unassigned axis (record nahi)
  // sample nahi hota — value 0 (avg_pedal kabhi 0xFF pin par nahi bulaya jata).
  s.accelAnalog = pinMap.accel != RSH_PINMAP_ABSENT
      ? avg_pedal(pinMap.accel, accelAnalogMode, lastAccelDiag) : 0;
  s.brakeAnalog = pinMap.brake != RSH_PINMAP_ABSENT
      ? avg_pedal(pinMap.brake, brakeAnalogMode, lastBrakeDiag) : 0;
  // FW-11 (F7): band module ka ADC read NAHI hota — handbrake band ho to
  // avg_pedal call hi nahi hota (koi ADC conversion, koi dummy read nahi),
  // value 0 rehti hai aur axis mask bit bhi clear hoti hai (modules.h).
  s.handbrake = (rsh_modules_handbrake_enabled() &&
                 pinMap.handbrake != RSH_PINMAP_ABSENT)
      ? avg_pedal(pinMap.handbrake, true, lastHandbrakeDiag) : 0;
  // Phase F WS5 (F7): clutch band ho to ADC read nahi hota (value 0,
  // axis mask bit bhi clear — modules.h). Clutch pin GPIO33 default
  // map mein gear-down bhi hai — clutch on ho to gear-down ka digital
  // read ADC read se takrata nahi (alag peripheral), lekin config
  // validator dono ko ek saath allow nahi karta (PIN_CONFLICT).
  // Phase G (C3): clutch pin blob se aata hai (kisi bhi ADC1 pin par).
  s.clutch = (rsh_modules_clutch_enabled() &&
              pinMap.clutch != RSH_PINMAP_ABSENT)
      ? avg_pedal(pinMap.clutch, true, lastClutchDiag) : 0;
  pedalCycle++;

  // FW-12: quadrature encoders — band module ke counts freeze (step nahi
  // hota) aur report 0 hoti hai; chalu module cumulative i16 count deta hai.
  // Phase F WS5: H-pattern runtime mode mein encoder pins shifter ke
  // hain — no read (enc values freeze nahi, 0 report hoti hain — neeche
  // else branch jaisi; mode wapas sequential hone par decoders wahi se
  // continue karte hain jahan ruke the — prev state mehfooz hai).
  // Phase G (C3): har encoder alag — sirf wohi step hota hai jis ke dono
  // pins blob mein hain (ek channel akela ho to pinmap use absent karta hai).
  s.enc[2] = 0;
  s.enc[3] = 0;
  if (rsh_modules_encoders_enabled() &&
      rsh_modules_shifter_mode() != RSH_SHIFTER_HPATTERN) {
    if (pinMap.enc1A != RSH_PINMAP_ABSENT && pinMap.enc1B != RSH_PINMAP_ABSENT) {
      rsh_encoder_step(&enc1, (uint8_t)digitalRead(pinMap.enc1A),
                       (uint8_t)digitalRead(pinMap.enc1B));
      s.enc[0] = enc1.count;
    } else {
      s.enc[0] = 0;
    }
    if (pinMap.enc2A != RSH_PINMAP_ABSENT && pinMap.enc2B != RSH_PINMAP_ABSENT) {
      rsh_encoder_step(&enc2, (uint8_t)digitalRead(pinMap.enc2A),
                       (uint8_t)digitalRead(pinMap.enc2B));
      s.enc[1] = enc2.count;
    } else {
      s.enc[1] = 0;
    }
  } else {
    s.enc[0] = 0;
    s.enc[1] = 0;
  }

  // FW-11: H-pattern shifter gear (sequential mode mein 0).
  // Phase F WS5: runtime mode check (compile-time #if nahi) — SET_CFG
  // blob se mode badalne par agle hi cycle se gear decode badal jata hai.
  // Phase G (C3): gear pins blob se; 0xFF pin daba hua nahi mana jata.
  s.shifterGear = 0;
  if (rsh_modules_shifter_mode() == RSH_SHIFTER_HPATTERN &&
      rsh_modules_shifter_enabled()) {
    uint8_t bits6 = 0;
    if (pinMap.shiftH[0] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[0]) == LOW) bits6 |= 0x01;
    if (pinMap.shiftH[1] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[1]) == LOW) bits6 |= 0x02;
    if (pinMap.shiftH[2] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[2]) == LOW) bits6 |= 0x04;
    if (pinMap.shiftH[3] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[3]) == LOW) bits6 |= 0x08;
    if (pinMap.shiftH[4] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[4]) == LOW) bits6 |= 0x10;
    if (pinMap.shiftH[5] != RSH_PINMAP_ABSENT &&
        digitalRead(pinMap.shiftH[5]) == LOW) bits6 |= 0x20;
    s.shifterGear = rsh_shifter_hpattern_gear(bits6);
  }

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
  }
  prevSteer = s.steering; prevAccel = s.accelAnalog; prevBrake = s.brakeAnalog;
  prevClutch = s.clutch;
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
  // mV mode mein steering rail ~3300 mV hai, raw mode mein 4095.
  uint16_t steerRail = useMv ? 3300 : 4095;
  axisInvalidMask = 0;
  // Phase G (C3): unassigned axis ka health check nahi hota — absent
  // axis ki value 0 hoti hai, jo rail-stuck jaisi dikhti hai; us par
  // invalid bit lagana ghalat hota (axis maujood hi nahi).
  if (pinMap.steer != RSH_PINMAP_ABSENT &&
      healthSteer.update(s.steering, steerRail, othersForSteer, nowMs))
    axisInvalidMask |= 0x01;
  if (pinMap.accel != RSH_PINMAP_ABSENT &&
      healthAccel.update(s.accelAnalog, 4095, othersForAccel, nowMs))
    axisInvalidMask |= 0x02;
  if (pinMap.brake != RSH_PINMAP_ABSENT &&
      healthBrake.update(s.brakeAnalog, 4095, othersForBrake, nowMs))
    axisInvalidMask |= 0x04;
  // Phase F WS5: clutch health bit 3. Band module ka clutch hamesha 0
  // hai — AxisHealth rail-stuck is par bhi chalega, lekin band module
  // ki bit ko .ino axis mask se hata deta hai (mask mein clutch bit
  // sirf enabled hone par set hoti hai), is liye mask mein ghalat
  // invalid bit nahi jati. Phase G (C3): unassigned clutch ka bhi
  // health check nahi hota (upar wali wajah).
  if (pinMap.clutch != RSH_PINMAP_ABSENT &&
      healthClutch.update(s.clutch, 4095, othersForClutch, nowMs))
    axisInvalidMask |= 0x08;

  // Buttons active-low hain (INPUT_PULLUP, doosri taraf GND).
  // Debounce time-based hai (F12) — governor level se aazad, hamesha 10 ms.
  // FW-11: band module ka button bit NAHI jata — shifter band ho to
  // gear up/down bits clear, horn band ho to horn bit clear (read bhi
  // nahi hota, F7 ki tarah).
  uint8_t b = 0;
  if (rsh_modules_shifter_enabled()) {
    // Phase G (C3): gear pins live table se; unassigned button ka
    // debouncer update nahi hota (bit kabhi nahi aata).
    if (pinMap.gearUp != RSH_PINMAP_ABSENT &&
        dbGearUp.update(digitalRead(pinMap.gearUp) == LOW, nowMs))   b |= RSH_BTN_GEAR_UP;
    if (pinMap.gearDown != RSH_PINMAP_ABSENT &&
        dbGearDown.update(digitalRead(pinMap.gearDown) == LOW, nowMs)) b |= RSH_BTN_GEAR_DOWN;
  }
  // Round 2 W3 (FIX-2): fixed pedal button pins (25/26) sirf tab parho
  // jab blob ne unhein kisi aur function ko na diya ho — warna ek hi
  // physical pin do button bits deta (misal encoder B pin 25 LOW hone
  // par accel-button bit bhi lag jata). rsh_pinmap_uses_pin pure hai,
  // host tests mein covered hai.
  if (!rsh_pinmap_uses_pin(&pinMap, PIN_ACCEL_BTN) &&
      dbAccel.update(digitalRead(PIN_ACCEL_BTN) == LOW, nowMs))  b |= RSH_BTN_ACCEL;
  if (!rsh_pinmap_uses_pin(&pinMap, PIN_BRAKE_BTN) &&
      dbBrake.update(digitalRead(PIN_BRAKE_BTN) == LOW, nowMs))  b |= RSH_BTN_BRAKE;
  if (rsh_modules_horn_enabled() && pinMap.horn != RSH_PINMAP_ABSENT &&
      dbHorn.update(digitalRead(pinMap.horn) == LOW, nowMs))    b |= RSH_BTN_HORN;
  s.buttons = b;
  return s;
}
