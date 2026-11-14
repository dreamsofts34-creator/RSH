# RSH — Reflash Guide (C++ firmware v4.0.1, Rust app v1.0.0)

**Tareekh:** 2026-11-14 · **Scope:** FW-A3
**Board:** ESP32 DevKit V1 (asal ESP32, 4 MB flash) · **Firmware:** C++ v4.0.1
**Rust app v1.0.0 ko koi reflash nahi chahiye** — protocol v2.1 frozen hai,
wahi board wahi firmware ke saath chalta rehta hai. Ye guide sirf **naye
board** (pehli dafa flash) ya **brick recovery** ke liye hai. Detail aur
authority: `docs/FIRMWARE-COMPAT.md` (compat matrix + verification evidence)
aur `reference/docs/BRICK-RECOVERY.md`.

## 0. Pehle ye samajh lo — kab reflash karo, kab nahi

| Haalat | Reflash? |
|---|---|
| Rust app v1.0.0 install / update kar rahe ho | **NAHI** — app update se firmware nahi badalta; board v4.0.1 par hi chalta hai |
| Naya (blank) ESP32 DevKit V1 board | **HAAN** — pehli dafa factory image flash karo (§3) |
| Board brick lagta hai (koi Wi-Fi nahi, boot nahi hota) | **HAAN** — §5 recovery, wahi factory image |
| Firmware v4.0.1 → v4.0.1 dobara flash | No-op — version wahi rehti hai; sirf tab karo jab image corrupt lage |
| Config kharab ho gayi / board ajeeb behave karta hai | Pehle app se **FACTORY_RESET** (opcode `0x08`) try karo — reflash nahi; factory reset key + Wi-Fi password **barkarar** rakhta hai |

## 1. Kya chahiye

| Cheez | Kahan se |
|---|---|
| Merged factory bin `RSH-Firmware-DevKitV1-v4.0.1.bin` (**955,056 B**) | GitHub Release (Rust v1.0.0 / RSH v2.0.4 assets) ya local `pio run` (`.pio/build/esp32dev/`) |
| `SHA256SUMS` (release ke saath) | GitHub Release assets — flash se pehle verify karo (§2) |
| `esptool.py` | PlatformIO ke saath (`~/workspace/projects/rsh/build/.venv-pio/bin/` mein `tool-esptoolpy`) ya `pip install esptool` |
| USB **data** cable (charge-only nahi) | — |
| CP210x / CH340 USB-serial driver (board ke chip ke mutabiq) | Vendor site / OS update |
| PC (Windows/Linux) | — |

**Ghalat file se recovery nahi hoti:** app-only `firmware.bin` (889,520 B)
se brick recovery **NAHI** hoti — wo `0x10000` par jati hai aur bootloader
ke baghair boot nahi hoti. Hamesha **merged** bin use karo (bootloader
`0x1000` + partitions `0x8000` + boot_app0 `0xe000` + app `0x10000`, ek hi
file, **offset `0x0`** par flash hoti hai). `firmware-signed.bin`
(889,588 B) sirf **OTA** ke liye hai (WebOTA/app se upload) — USB flashing
ke liye merged bin chahiye.

## 2. Flash se pehle — verify karo

```bash
# Linux
sha256sum RSH-Firmware-DevKitV1-v4.0.1.bin
# SHA256SUMS mein di gayi value se milao — na mile to file dobara download karo, flash mat karo.
# Windows (PowerShell)
Get-FileHash RSH-Firmware-DevKitV1-v4.0.1.bin -Algorithm SHA256
```

- Size **955,056 B** hona chahiye. Chhoti file (e.g. 889,520 B
  `firmware.bin`) = ghalat file, flash mat karo.
- Release build ka SHA-256 GitHub Release page par `SHA256SUMS` mein hota
  hai. Local `pio run` se bani file ka hash is doc mein darj run se mukhtalif
  ho sakta hai (build environment) — size + structure (§5 ka host-mock)
  check karo.

## 3. Flash karna (naya board ya dobara flash)

1. **Board ko USB se connect karo.** Device Manager (Windows) ya
   `ls /dev/ttyUSB*` (Linux) mein COM port / tty device nazar aaye. Na aaye
   to: data cable badlo, phir driver install karo, phir doosra USB port try
   karo (peeche wale ports zyada stable hote hain).
2. **Download mode:** board par **BOOT** button dabaye rakho, **EN/RST** ek
   dafa dabao-chhoro, phir BOOT chhoro. (Kuch boards par sirf BOOT hold +
   connect kaafi hai.)
3. **Flash karo** (port apna daalo):

   ```bash
   # Linux
   esptool.py --chip esp32 --port /dev/ttyUSB0 --baud 921600 \
     write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.1.bin

   # Windows (port apna daalo — COM3, COM4 …)
   esptool.py --chip esp32 --port COM3 --baud 921600 \
     write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.1.bin
   ```

   Baud fail ho to `115200` try karo. **Offset `0x0` lazmi hai** — koi aur
   offset board ko boot nahi hone dega.
4. **Verify:** esptool aakhir mein `Hash of data verified` likhta hai. Na
   likhe to dobara flash karo (cable/port badal kar).
5. **Restart:** EN/RST dabao ya USB dobara lagao.
6. **Confirm:** PC/phone ki Wi-Fi list mein **`RSH`** network nazar aaye —
   matlab firmware chal raha hai. Phir Rust app v1.0.0 se connect karo:
   app HELLO karegi, board firmware **v4.0.1** + protocol **v2.1** report
   karega (app ke Device/Output page par nazar aata hai).

## 4. Source se build karke flash karna (developers)

```bash
export TMPDIR=~/tmp   # /tmp 512 MB tmpfs hai — bare builds ke liye ye lazmi hai
cd <firmware copy>    # reference/ mein kabhi build mat karo — scratch copy use karo
~/workspace/projects/rsh/build/.venv-pio/bin/pio run
# Output: .pio/build/esp32dev/RSH-Firmware-DevKitV1-v4.0.1.bin (955,056 B)
# Upload (board download mode mein ho):
~/workspace/projects/rsh/build/.venv-pio/bin/pio run -t upload
# Serial monitor (sirf jab config.h mein RSH_DEBUG 1 ho):
~/workspace/projects/rsh/build/.venv-pio/bin/pio run -t monitor   # 115200 baud
```

- `pio run` ke baad merged bin khud ban jata hai (`merge_bin.py` post-script).
  Merged bin < `0x10000 + 1024` B ho to build fail hota hai (F15).
- **Signing (release ke liye):** `RSH_SIGNING_KEY=<Ed25519 private key PEM ka
  path>` env var set ho to `sign_firmware.py` `firmware-signed.bin`
  banata hai (`firmware.bin` + 68 B trailer). Key set nahi → signing skip,
  dev build unsigned rehta hai; key set lekin file nahi / sign fail → build
  zor se fail hota hai. **Key kabhi repo mein nahi aati** — private key
  `~/workspace/projects/rsh/signing-keys/` mein hai (repo se bahar, 0600)
  aur CI secret `RSH_SIGNING_KEY_PEM` mein; public-key fingerprint
  `SHA-256 387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`.
  Key ki value kabhi print, copy ya share mat karo — sirf path use karo.

## 5. Flashing fail ho jaye to — recovery

Board asal mein brick **nahi** hota: ESP32 ka ROM bootloader hamesha rehta
hai, is liye USB se factory image dobara flash karne se board wapas aata hai.

| Masla | Kya karo |
|---|---|
| No COM port / device nazar nahi aata | Cable → driver → USB port (peeche wale ports try karo) |
| `Failed to connect` | BOOT-button trick (§3 step 2) + baud `115200` par dobara |
| Flash beech mein ruk jaye (stall) | Cable/port badlo, dobara karo — adhoora flash board ko nuqsan nahi pahunchata (bootloader ROM mein hai) |
| Flash ho gaya, `RSH` Wi-Fi nahi | EN/RST dabao; phir bhi nahi to offset check karo — merged bin ka offset **0** hona chahiye, aur file merged wali honi chahiye (`firmware.bin` nahi) |
| Sab kuch fail | **Aakhri rasta:** `esptool.py erase_flash` karke §3 dobara karo (NVS/config samet sab mit jata hai — board factory state par aata hai; pairing dobara karni hogi) |

### 5.1 BOOT-hold safe mode (kharab config se recovery, bina reflash)

Safe mode **runtime** detect hota hai (power-up par nahi — loop har
iteration GPIO0 / BOOT button parhta hai):

| Hold | Natija |
|---|---|
| 5 s ≤ hold < 10 s | **Safe mode** — factory pin map + AP; config apply nahi hoti (stage + commit `BUSY (7)` se roke jate hain) |
| hold ≥ 10 s | **Key-reset-armed** (KEY_RESET ke liye — sirf is state mein `0x0B` chalta hai) |

- Boot counter: kamyab boot (AP waqt par up, 15 s deadline) dono counter
  reset karta hai; lagatar fail boots par board Last-Known-Good (LKG) config
  par wapas jata hai, phir factory fallback.
- **Reboot kabhi unconfirmed config ko confirm nahi karta** — boot par
  pending-confirm rollback hota hai (D28). Is liye kharab config kabhi board
  ko brick nahi karti: reboot karo, safe mode ya LKG/factory config se board
  wapas aata hai, phir app se sahi config dobara bhejo.
- Safe mode mein sirf AP + WebOTA chalte hain — WebOTA se signed
  `firmware-signed.bin` upload karke bhi recover kiya ja sakta hai (board
  verify-before-apply karta hai; unsigned ya tampered image reject hoti hai).

## 6. Release process (signed firmware)

1. `pio run` — build + merged bin (955,056 B) + `firmware-signed.bin`
   (889,588 B, sirf key maujood ho to).
2. Release workflow mein **fail-closed verify**: signature verify hoti hai
   aur signed-region hash `firmware.bin` se milaya jata hai — verify fail →
   release ruk jati hai. Tampered ya downgrade image board par reject hoti
   hai (Ed25519 verify-before-apply + anti-downgrade).
3. Release assets (GitHub Release): merged bin
   `RSH-Firmware-DevKitV1-v4.0.1.bin` + `firmware-signed.bin` + `SHA256SUMS`.
   Signing keypair **existing wala reuse** hota hai — naya keypair nahi
   banta.
4. **App release (Tauri updater) ka signing alag hai** — us ki dedicated
   Ed25519 keypair hai (`TAURI_SIGNING_PRIVATE_KEY` CI secret); firmware
   key, Tauri updater key aur (deferred) OV code-signing certificate
   **teen alag** cheezein hain (detail: `docs/KEY-LIFECYCLE-RUST.md`).

## 7. Flash ke baad checklist

- [ ] `RSH` Wi-Fi nazar aata hai
- [ ] Rust app v1.0.0 connect hoti hai (HELLO OK)
- [ ] App firmware **v4.0.1** + protocol **v2.1** dikhati hai
- [ ] Output page par axes/buttons live update hote hain (200 Hz stream)
- [ ] Pehli pairing (agar nayi board / `erase_flash` ke baad): pairing
      window SoftAP up hone se **60 s** khulti hai — usi doran app se pair
      karo; window band hone ke baad key/password nahi milte
