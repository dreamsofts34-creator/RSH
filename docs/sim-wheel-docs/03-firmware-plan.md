# 03 - Firmware Plan (ESP32-C3 Super Mini, Arduino)

## Toolchain

- Arduino IDE 2.x + "esp32 by Espressif" board package
- Board: **ESP32C3 Dev Module**
- **USB CDC On Boot: Enabled** (warna Serial monitor khali dikhega)
- Upload na ho to: BOOT dabaye rakho, USB lagao, phir upload

## Files

```
firmware/
  firmware.ino      setup/loop, scheduler
  config.h          pins, SSID, ports, rates, TX power
  inputs.h/.cpp     ADC averaging, button debounce
  protocol.h/.cpp   packet struct, CRC16
  net.h/.cpp        SoftAP, UDP, handshake
  status_led.h/.cpp LED patterns
```

## config.h (idea)

```cpp
#define PIN_STEER      0
#define PIN_ACCEL_BTN  5
#define PIN_BRAKE_BTN  6
#define PIN_ACCEL_ADC  3
#define PIN_BRAKE_ADC  4
#define PIN_GEAR_UP    7
#define PIN_GEAR_DOWN  10
#define PIN_LED        8
#define LED_ACTIVE_LOW 1

#define SEND_PERIOD_US 5000
#define STEER_SAMPLES  16
#define PEDAL_SAMPLES  8
#define DEBOUNCE_SAMPLES 2        // 2 x 5 ms = 10 ms

#define WIFI_SSID      "SIMWHEEL"
#define WIFI_PASS      "change-me-123"
#define WIFI_CHANNEL   1
#define TX_POWER       WIFI_POWER_8_5dBm
#define PORT_HELLO     4210
#define PORT_DATA      4211
#define HELLO_TIMEOUT_MS 2000
```

## Inputs

- `analogReadResolution(12)`; attenuation 11 dB (default).
- **Steering**: pot ke upar ~15 kOhm series resistor (hardware fix, `01-verification.md` #5). Phir bhi step 1 mein dono compare karo: `analogRead` raw vs `analogReadMilliVolts` (factory calibration use karta hai, saturation/nonlinearity behtar). Jo behtar linear nikle use karo, aur packet flags bit0 set karo.
- 16 samples ka average har cycle. Pedals 8 samples. Har ADC pin sirf ek baar read, taake time budget 5 ms ke andar rahe (step 1 mein `micros()` se naapo, target < 1.5 ms total).
- Dono pedal types hamesha bhejo: button bits **aur** analog raw. Floating ADC pin ka kachra PC ignore karega (config mein `mode: button`).
- Buttons: `INPUT_PULLUP`, active-low, 2 lagataar same samples = state change.

## Network

```cpp
WiFi.mode(WIFI_AP);
WiFi.softAP(WIFI_SSID, WIFI_PASS, WIFI_CHANNEL, 0, 2);
WiFi.setTxPower(TX_POWER);   // softAP start ke BAAD
WiFi.setSleep(false);
udp.begin(PORT_HELLO);       // HELO sunne ke liye
```

- `HELO` aane par `pcIP` aur `lastHelloMs` update.
- Data: `udp.beginPacket(pcIP, PORT_DATA); udp.write(buf, 18); udp.endPacket();`
- `millis() - lastHelloMs > HELLO_TIMEOUT_MS` to bhejna band.
- TX power ko constant rakho; agar aapke board par default power par bhi chal jaye to test karke dekho (kam power = kam range, desk par farq nahi parta).
- Pin 20/21 par kuch mat lagao (Super Mini WiFi masla report hua hai).

## Scheduler

- `loop()` non-blocking: `if ((int32_t)(micros() - nextTick) >= 0) { nextTick += SEND_PERIOD_US; sample(); send(); }`
- Serial prints release build mein nahi (debug flag ke peeche).
- Overrun counter rakho (agar cycle 5 ms se zyada le).

## Status LED

| State | Pattern |
|---|---|
| Boot / AP up, PC nahi | blink 2 Hz |
| PC HELO active | solid on |
| Error (WiFi start fail) | tez blink 8 Hz |

## Debug mode (compile flag)

1 Hz Serial: seq, send rate, overrun count, steering min/max/stddev pichle second ka. Isi se step 1 ka noise test hota hai.

## Milestones

1. **Step 0**: sirf SoftAP + HELO echo; PC se connect, ping, 5 min stable?
2. ADC read + Serial, noise numbers (WiFi on aur off dono)
3. Packet build + CRC, Simulator ya console harness se parse
4. Buttons + debounce
5. LED states, fail-safes
6. 200 pps soak 30 min

## Sampling jitter note

Averaging noise kam karta hai lekin range galat ho ya grounding kharab ho to theek nahi karta. Pot wires chhoti, common ground ek hi point par, WiFi antenna se door.
