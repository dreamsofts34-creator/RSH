# Brick recovery — ESP32 DevKit V1 (FW-14, R-M12)

> Board "brick" hone ka matlab: koi bhi firmware boot nahi hota (galat
> flash, adhoori OTA, kharab config). ESP32 ka ROM bootloader hamesha
> rehta hai — is liye board asal mein brick nahi hota: USB se factory
> image dobara flash karne se wapas aata hai. Ye doc wahi procedure hai.

## Required files (sab lazmi)

| Cheez | Kahan se |
|---|---|
| Merged factory bin `RSH-Firmware-DevKitV1-v4.0.0.bin` | GitHub Release v2.0.0 (assets) ya local `pio run` (`.pio/build/esp32dev/`) |
| `esptool.py` | PlatformIO ke saath (`build/.venv-pio` mein `tool-esptoolpy`) ya `pip install esptool` |
| USB **data** cable (charge-only nahi) | — |
| CP210x / CH340 USB-serial driver (board ke chip ke mutabiq) | vendor site / OS update |
| PC (Windows/Linux) | — |

Merged bin ki pehchan: bootloader (0x1000) + partitions (0x8000) +
boot_app0 (0xe000) + app (0x10000) — ek hi file, **offset 0x0** par flash
hoti hai. App-only `firmware.bin` se recovery NAHI hoti (wo 0x10000 par
jati hai aur bootloader ke baghair boot nahi hota).

## Recovery steps

1. **Board ko USB se connect karo.** Device Manager / `ls /dev/ttyUSB*`
   mein COM port / tty device nazar aaye. Na aaye to: data cable badlo,
   phir driver install karo, phir doosra USB port try karo.
2. **Download mode:** board par **BOOT** button dabaye rakho, **EN/RST**
   ek dafa dabao-chhoro, phir BOOT chhoro. (Kuch boards par sirf BOOT
   hold + connect kaafi hai.)
3. **Flash karo** (port apna daalo):
   ```bash
   esptool.py --chip esp32 --port /dev/ttyUSB0 --baud 921600 \
     write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.0.bin
   ```
   Windows par port `COM3` jaisa hota hai. Baud fail ho to `115200` try karo.
4. **Verify:** esptool aakhir mein `Hash of data verified` likhta hai.
   Na likhe to dobara flash karo (cable/port badal kar).
5. **Restart:** EN/RST dabao ya USB dobara lagao.
6. **Confirm:** PC/phone ki Wi-Fi list mein `RSH` network nazar aaye —
   matlab firmware chal raha hai. Na aaye to step 2 se dobara karo.

## Agar phir bhi na chale

- **No COM port:** cable → driver → USB port (peeche wale ports zyada stable).
- **"Failed to connect":** BOOT-button trick + baud 115200.
- **Flash stalls mid-way:** cable/port badlo, dobara karo — adhoora flash
  board ko nuqsan nahi pahunchata (bootloader ROM mein hai).
- **Flash ho gaya, RSH Wi-Fi nahi:** EN/RST dabao; phir bhi nahi to
  offset check karo — merged bin ka offset **0** hona chahiye, aur file
  merged wali honi chahiye (`firmware.bin` nahi).
- **Aakhri rasta:** `esptool.py erase_flash` karke step 3 dobara karo
  (NVS/config samet sab mit jata hai — board factory state par aata hai).

## Host-mock test (bina board ke)

Recovery ka host-mock test `tests-host/test_brick_recovery.py` hai — wo
merged bin ki structure validate karta hai (bootloader magic 0xE9 @ 0x1000,
partition magic 0xAA50 @ 0x8000, app magic 0xE9 @ 0x10000) aur recovery
command banata hai. Board ke baghair chalta hai:

```bash
python3 tests-host/test_brick_recovery.py --merged-bin \
  firmware/rsh/.pio/build/esp32dev/RSH-Firmware-DevKitV1-v4.0.0.bin
```

Mock test pass hona lazmi hai — is ka matlab hai ke release/build mein
mojood merged bin asal mein flash karne ke qaabil hai.
