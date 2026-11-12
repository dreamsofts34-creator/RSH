# Sim Wheel (ESP32-C3 Super Mini + C# .NET 10 WPF + vJoy)

WiFi se jura steering wheel: ESP32-C3 raw values UDP par bhejta hai; Windows par SimWheel app (C#, WPF) calibrate karke vJoy device banati hai.

## Files

| File | Kya hai |
|---|---|
| `01-verification.md` | Plan ka research-based check, corrections, v2 stack decisions, sources |
| `02-architecture.md` | System design, stack, solution layout, packet format, vJoy mapping, failure behaviour |
| `03-firmware-plan.md` | ESP32 firmware: config, inputs, WiFi, scheduler, milestones |
| `04-software-plan.md` | C# app: modules, vJoy backend, GUI pages, profiles, testing, milestones |
| `05-setup.md` | Wiring, flashing, vJoy + app setup, developer setup, pehli baar chalana |
| `06-testing-troubleshooting.md` | Har step ka pass criteria aur masle/hal |

## Sabse zaroori faislay

1. **.NET 10 LTS** (.NET 8/9 ka support 10 Nov 2026 ko khatam).
2. **WPF + WPF-UI 4.x**: modern Fluent look, tray, autostart.
3. **vJoy** output (primary), apni P/Invoke layer; DLL install folder se.
4. Steering pot ke upar ~15 kOhm series resistor (C3 ADC saturation).
5. UDP unicast + handshake (broadcast nahi).
6. Simulator tool: hardware ke bina app test.

Shuruaat: `01` -> `05` -> `03` -> `04` -> `06`.
