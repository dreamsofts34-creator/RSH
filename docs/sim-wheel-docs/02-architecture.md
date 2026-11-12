# 02 - Architecture (v2: C# .NET 10 + WPF + vJoy)

## Overview

```
[Steering pot] --\
[Pedal buttons /  \
 49E hall]  -------> [ESP32-C3 Super Mini] ==WiFi (SoftAP, UDP unicast)==> [Windows PC]
[Gear buttons] ---/     (raw values only)                                  SimWheel app (C#)
                                                                              |
                                                                  calibration + filters + profiles
                                                                              |
                                                                  vJoy driver (DirectInput device)
                                                                              |
                                                                            Game
```

Principle: **firmware dumb, PC smart**. ESP32 sirf raw values bhejta hai; calibration, curves, ramp sab PC par, taake reflash na karna pare.

## Stack decisions (aur kyun)

| Cheez | Choice | Wajah |
|---|---|---|
| Language/runtime | **C# on .NET 10 LTS** | .NET 8/9 ka support 10 Nov 2026 ko khatam; .NET 10 Nov 2028 tak supported |
| UI | **WPF + WPF-UI 4.x** (Fluent, dark/light, Mica) | WPF stable aur mature; WPF-UI .NET 10 support karta hai |
| MVVM | CommunityToolkit.Mvvm | Standard, kam boilerplate |
| Graphs | ScottPlot 5 | Live plots ke liye |
| Output | **vJoy** (DirectInput) | Alag axes, 16-bit steering, zyada buttons |
| vJoy access | Apni thin P/Invoke layer (`vJoyInterface.dll`), install folder se load | Wrapper/driver version mismatch se bachne ke liye |
| Config | System.Text.Json, atomic write | Crash-safe profiles |
| Logging | Serilog (rolling file) | Field mein debug |
| Packaging | Self-contained single-file `win-x64` + Inno Setup installer | User ko .NET install nahi karna |

## Solution layout

```
SimWheel.sln
  src/SimWheel.Core        (net10.0)  protocol, transport, calibration, processing, profiles, output abstraction
  src/SimWheel.Output.VJoy (net10.0)  vJoy backend (P/Invoke)
  src/SimWheel.App         (net10.0-windows) WPF UI, tray, autostart, DI host
  tools/SimWheel.Simulator (net10.0)  fake ESP32 UDP sender (bina hardware ke test)
  tests/SimWheel.Tests     xUnit: protocol, CRC, processing, calibration, replay
```

`IOutputBackend` interface: abhi `VJoyBackend`; baad mein chaho to `ViGEmBackend` bina core badle.

## Hardware map

| Function | Pin | Note |
|---|---|---|
| Steering pot wiper | GPIO0 (ADC1) | 0.1 uF wiper-GND, pot top par ~15 kOhm series |
| Accelerator button / hall | GPIO5 (digital) / GPIO3 (ADC1) | button = INPUT_PULLUP to GND |
| Brake button / hall | GPIO6 (digital) / GPIO4 (ADC1) | same |
| Gear up / down | GPIO7 / GPIO10 | INPUT_PULLUP |
| Status LED | GPIO8 (onboard, aksar active-low) | verify |
| Avoid | GPIO2, 9, 18, 19, 20, 21 | |

## Network design

- ESP32 **SoftAP** `SIMWHEEL`, WPA2, fixed channel (1/6/11), max 2 clients, ESP = `192.168.4.1`.
- **Handshake**: PC har 1 sec `HELO` `192.168.4.1:4210` par bhejta hai; ESP sender IP yaad rakh kar **unicast** data `PC:4211` par bhejta hai. 2 sec HELO na aaye to ESP chup.
- 200 packets/sec. Optional baad mein: USB CDC serial transport (wahi packet format).
- SoftAP par PC ka internet WiFi se nahi milta: Ethernet use karo.

## Packet format (little-endian, 18 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | u8 | magic `0xA5` |
| 1 | u8 | version (`1`) |
| 2 | u16 | sequence (wraps) |
| 4 | u32 | ESP timestamp (us) |
| 8 | u16 | steering raw |
| 10 | u16 | accelerator analog raw |
| 12 | u16 | brake analog raw |
| 14 | u8 | buttons: bit0 gear up, bit1 gear down, bit2 accel btn, bit3 brake btn |
| 15 | u8 | flags: bit0 = steering in mV |
| 16 | u16 | CRC16-CCITT (poly 0x1021, init 0xFFFF) over bytes 0-15 |

C#: `BinaryPrimitives.ReadUInt16LittleEndian` etc., `ReadOnlySpan<byte>`, koi allocation nahi.

## PC runtime pipeline

```
UDP thread (blocking ReceiveFrom, high priority, reused buffer)
  -> validate (length, magic, version, CRC, seq wrap-safe)
  -> latest-state (lock-free single slot)
  -> Processing: calibration -> deadzone -> curve -> filter (One Euro) -> ramp (button pedals)
  -> IOutputBackend.Update(state)  -> vJoy UpdateVJD
Watchdog thread (10 ms tick): 100 ms koi valid packet nahi -> neutral
UI thread: sirf 30-60 Hz par state dikhata hai, data path ko block nahi karta
```

Startup par `timeBeginPeriod(1)`; process priority Above Normal; GC pressure kam (no per-packet allocations).

## vJoy axis mapping (default, GUI mein badal sakte ho)

| Input | vJoy |
|---|---|
| Steering | X axis |
| Accelerator | Y axis |
| Brake | Z axis |
| Gear up / down | Button 1 / 2 |
| Spare | Rx/Ry (clutch), buttons 3-32 |

vJoy device ka configuration (axes, buttons) `vJoyConf` se karna parta hai (`05-setup.md`).

## Failure behaviour

| Situation | Result |
|---|---|
| Late/duplicate/corrupt packet | Drop, counter |
| WiFi band | 100 ms baad neutral, UI + tray "Disconnected", auto reconnect |
| ESP reboot | Seq reset detect |
| vJoy driver missing/mismatch | App chalti hai, "Output unavailable" warning, setup guide link |
| vJoy device kisi aur ne acquire kar liya | Clear error, retry |
| Config corrupt | Last good backup se restore |
| App crash | Axes neutral par chhute (vJoy device relinquish), log file |

## Mechanical

- 4:1 gear (chhota gear steering shaft par). Backlash kam.
- Wheel par **+-450°** physical stop; pot ka internal stop par nirbhar mat raho.
- ESP32 antenna metal/motor/wires se door.

## Future

Hall pedals (sirf wiring + profile change), ViGEm backend, USB transport, force feedback (alag hardware chahiye).
