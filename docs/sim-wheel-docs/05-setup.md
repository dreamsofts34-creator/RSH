# 05 - Setup Guide

## 1. Parts

- ESP32-C3 Super Mini
- B50K linear pot + 4:1 gear pair
- 0.1 uF capacitor, ~15 kOhm resistor (pot ke series)
- 4 push buttons (2 pedal, 2 gear)
- Baad mein: 2x 49E hall sensor + magnets
- Wires, common ground

## 2. Wiring

**Steering pot**

```
3V3 --[15k]-- pot pin 1
GND ---------- pot pin 3
wiper (pin 2) -- GPIO0
0.1 uF between wiper and GND (ESP pins ke bilkul paas)
```

Series resistor ki wajah se wiper ka voltage ~0.2V-2.3V rehta hai (225° travel par), jo ADC ke linear hisse mein hai. Direction ulti ho to PC config mein `invert`.

**Buttons** (sab ek taraf GND, doosri taraf GPIO, firmware `INPUT_PULLUP`)

| Button | GPIO |
|---|---|
| Accelerator | 5 |
| Brake | 6 |
| Gear up | 7 |
| Gear down | 10 |

**Hall sensors (baad mein)**: VCC -> **3V3**, GND -> GND, OUT -> GPIO3 (accel) / GPIO4 (brake). 5V se power mat karo. Datasheet check karo.

Onboard BOOT (GPIO9) par kuch mat lagao.

## 3. Mechanical checklist

- Pot center par, wheel center ke saath align
- Gear mesh tight, backlash kam
- Wheel par +-450° ka physical stop (pot end-stop par nirbhar mat raho)
- ESP32 ka antenna end metal aur motor/wires se door

## 4. Firmware flash

1. Arduino IDE 2.x install, Boards Manager se "esp32 by Espressif"
2. Board: ESP32C3 Dev Module, USB CDC On Boot: Enabled
3. `config.h` mein SSID/password badlo
4. Upload (na ho to BOOT dabaye rakh kar USB lagao)
5. LED 2 Hz blink = AP up, PC ka intezar

## 5. PC setup

1. **vJoy install** (version 2.1.9.1 recommended; Windows 11 par signed driver ke saath kaam karta bataya gaya hai). Install ke baad reboot.
2. **vJoyConf** kholo: Device 1 enable; axes **X, Y, Z** (aur chaho to Rx, Ry); Buttons **32**; Apply.
3. SimWheel app install karo (self-contained, .NET alag install nahi). Pehli baar vJoy detect check hota hai.
4. WiFi se `SIMWHEEL` join karo.
5. Windows Firewall: network **Private**, SimWheel ko allow karo (UDP 4211 inbound).
6. WiFi adapter power saving band: Device Manager -> adapter -> Properties -> Power Management -> "Allow the computer to turn off this device" untick. Power plan -> Wireless Adapter Settings -> Maximum Performance.
7. Internet ke liye Ethernet ya doosra adapter (SoftAP par internet nahi).
8. Settings mein "Start with Windows" + "Start minimized" on karo.

**vJoy masle:** driver aur DLL version mismatch ho to app warning dikhati hai. Test-signed vJoy builds ke liye Windows TESTSIGNING chahiye; kuch anti-cheat games us mode mein nahi chalte, isliye signed build hi use karo.

**Option B (baad mein)**: ESP32 ko STA mode mein router/phone hotspot se jodna (latency router par nirbhar).

## 6. Developer setup (app banane ke liye)

1. .NET 10 SDK install (`dotnet --version` 10.x)
2. Visual Studio (latest, ".NET desktop development") ya Rider
3. `dotnet new sln`, projects `Core`, `Output.VJoy`, `App`, `Simulator`, `Tests` (layout: `02-architecture.md`)
4. NuGet: `WPF-UI`, `CommunityToolkit.Mvvm`, `ScottPlot.WPF`, `Serilog.Sinks.File`, `Microsoft.Extensions.Hosting`, `xunit`
5. Hardware ke bina test: `SimWheel.Simulator` chalao (fake ESP32), app ko `127.0.0.1` par point karo

## 7. Pehli baar chalana

1. ESP32 power, LED blink -> app khol kar Dashboard par "Connected" dekho (LED solid)
2. Dashboard par pps ~200, loss < 1%
3. **Calibration** page: steering left/center/right, pedals
4. `joy.cpl` -> vJoy Device -> Properties: axes smooth chalte hain?
5. Game mein controller select karo (vJoy device), axes bind karo

## 8. Game mein

1. Pehle app ke curve/lock se tune karo, game mein deadzone 0, saturation 100
2. Steering lock 900° ya game ke rotation setting ke hisaab se
3. Pehle single-player racing game
4. Masla ho to `06-testing-troubleshooting.md`
