# 06 - Testing & Troubleshooting

## Build order aur pass criteria

| Step | Test | Pass |
|---|---|---|
| 0 | SoftAP up, PC join, ping `192.168.4.1` 5 min | Koi disconnect nahi, ping stable |
| 1 | Pot + cap, Serial par ADC (WiFi **on** rakh kar) | Hilaye bina 16-avg ±6 counts ke andar; wheel ke poore travel mein value monotonic, upar flat nahi |
| 1b | ADC read time `micros()` se | 16+8+8 samples < ~1.5 ms |
| 2 | UDP packets PC par | ~200 pps (±5%), loss < 1%, p99 inter-arrival < 15 ms, bad CRC ~0 |
| 3 | Calibration + vJoy output | joy.cpl mein vJoy Device ka X axis poora left-right smooth, Y/Z pedals alag |
| 3b | Watchdog | ESP USB nikalo -> 100-150 ms mein sab neutral |
| 4 | Gears + physical stops | Stops par pot ka end stop nahi chhuta; gear click edge sahi |
| 5 | Button pedals ramp | 0 se 100% ~0.3 s, chhodne par girta hai |
| 5a | Simulator (bina hardware) | App 24 ghante simulator ke saath stable, memory/handle flat |
| 5b | Hall pedals (baad mein) | Config `mode: analog`, ramp band, range calibrate |
| 6 | Game tuning | Steering linear lage, center par jitter nahi |
| 7 | Soak 30 min | Koi freeze/disconnect nahi |

## Useful measurements

- Loss: sequence gaps / total
- Jitter: PC-side inter-arrival p50/p99
- Reboot detect: sequence ka achanak chhota ho jana

## Troubleshooting

| Problem | Pehle ye try karo |
|---|---|
| WiFi se connect nahi / ruk ruk kar | `WiFi.setTxPower(WIFI_POWER_8_5dBm)` **softAP ke baad**; antenna clear rakho; fixed channel 1/6/11; pin 20/21 khali; doosra board revision |
| Packets jhund mein aate hain | `WiFi.setSleep(false)`; PC adapter power saving band; broadcast ki jagah unicast; channel badlo |
| Loss zyada | Board PC ke paas, metal se door; 2.4 GHz ka kam bhida channel |
| Steering upar ki taraf flat | Series resistor lagao / `analogReadMilliVolts`; pot range check |
| Steering jitter | Cap pins ke paas, wires chhoti, ground ek point, smoothing badhao, WiFi on rakh kar test |
| Steering direction ulti | Config `invert` |
| vJoy device nahi dikhta | Device Manager mein vJoy check, vJoyConf mein Device 1 enable, reboot |
| "Driver/DLL version mismatch" | App install folder wali DLL use karti hai; vJoy dobara install karo, purane copies hatao |
| vJoy install fail (Win11) | Signed build (2.1.9.1) try karo; test-signed builds se bacho |
| Output "busy" | Koi doosra feeder app device acquire kar raha hai; band karo |
| App timing choppy | `timeBeginPeriod(1)`, UI aur data thread alag, logging level kam |
| Game kuch nahi pakadta | Game mein vJoy device selected? Game sirf XInput samajhta ho to vJoy nazar nahi aata (x360ce ya baad mein ViGEm backend). Kuch anti-cheat wale games virtual driver block karte hain |
| Sirf kuch seconds chal kar rukta | Firewall Private network, HELO keepalive chal raha? |

## Final acceptance

- 30 min soak, loss < 1%, koi spurious button/steering jump nahi
- Disconnect par neutral aur reconnect auto
- Calibration JSON restart ke baad bhi sahi
