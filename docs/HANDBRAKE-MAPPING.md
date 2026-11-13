# RSH — Handbrake mapping decision (Phase G, C8)

**Status: DECIDED 2026-11-13 (Phase G Workstream 3).**
**Decision: Handbrake → vJoy `WSlider` axis (analog).**

## Chain (pehle se maujood — koi tabdeeli nahi)

Firmware slot 4 → pipeline `HandbrakeNorm` / `HandbrakeAxis`
(`ProcessingPipeline.HandbrakeSlot = 4`, analog pot, mV) → vJoy output.
Sirf aakhri qadam (vJoy field) Phase G mein tay hua hai; firmware aur
pipeline pehle se slot 4 par thay (host tests §23, `PhaseFWorkstream5Tests`
`Handbrake_Slot4_NotSlot3`).

## Faisla

| Cheez | Value |
|---|---|
| vJoy field | `JoystickPosition.WSlider` (`VJoyOutput.BuildPosition`) |
| Neutral (chhora hua) | `InputMapper.AxisMin` (1) — pedals jaisa, center nahi |
| Full pull | `32768` (axis max) |
| Caps gating | Sirf jab device mein Slider axis ho (`VJoyCapabilities.HasAxisSlider`, HID usage `0x36`); na ho to neutral rehta hai |
| Validation (D11) | Handbrake enabled + Slider na ho → error: "Handbrake module on hai lekin vJoy device mein Slider axis nahi hai." |
| Setup | vJoyConf mein device 1 par **X, Y, Z + Slider** enable karna lazmi hai (USER-GUIDE §2); game mein handbrake → vJoy Slider bind karo |

## Kyun Slider?

- **vJoy mein handbrake ka koi dedicated axis nahi.** V3 driving axes
  sirf chaar hain: Steering, Accelerator, Brake, Clutch — sab pehle se
  kisi module ko mile hue hain. Handbrake ko dedicated axis dena mumkin
  hi nahi.
- **Slider generic 1-D axis hai** — HID Generic Desktop usage `0x36`
  (X `0x30`, Y `0x31`, Z `0x32`, Rx `0x33`, Ry `0x34`, Rz `0x35`,
  Slider `0x36`, Dial `0x37`, Wheel `0x38`). Games kisi bhi DirectInput
  axis ko handbrake ke tor par bind kar sakte hain; Slider is maqsad ke
  liye sim-racing vJoy setups mein aam istemal hota hai.
- **Analog barkarar rehta hai** — handbrake analog pot hai; gradual pull
  (drift/rally) ka matlab hai. Button mapping ye gradation kho deta.
- **Caps se poocha ja sakta hai** — Slider ki HID usage hoti hai, is liye
  `GetVJDAxisExist` se device mein is ki maujoodgi verify hoti hai aur
  D11 validation lagti hai. Ye Clutch se farq hai: `WClutch` ka koi HID
  usage nahi, is liye clutch caps se bahar rehta hai (Phase F WS6).
- **Koi takrao nahi** — Slider pehle kisi module mein istemal nahi hota
  tha (sirf neutral center par tha); encoder pulses buttons 65..72 par
  hain, wire buttons 1..64 par — Slider se koi overlap nahi.

## Mustarad options

| Option | Kyun nahi |
|---|---|
| Button / digital threshold (e.g. 50% se oopar = button dabba) | Analog gradation khatam; threshold arbitrary hota; do modules (analog + button) ki dohri mapping banti |
| Rx / Ry / Rz (rotation axes) | Rotation semantics ghalat hain; games inhein steering jaisi rotation samajhte hain |
| Throttle / Rudder / Aileron | Flight axes — naam aur semantics dono ghalat; game UI mein uljhan |
| Wheel / Dial | Wheel steering jaisa lagta hai; Dial kam aam aur kam games mein bind hota hai |
| Koi mapping nahi (pehle wali halat) | Handbrake games ko kabhi nazar na aata — module bekaar rehta; user ne hardware lagaya ho to bhi kaam na karta |

## Asar

- `VJoyOutput.NeutralPosition`: `WSlider` center se **min** hua — center
  aadha handbrake hota. Slider pehle unused tha, is liye kisi aur module
  par asar nahi.
- UI labels (truthful rehne ke liye): Tuning card subtitle ab "Analog
  lever · vJoy Slider" hai (CalibrationTuning.cs + 3 resx, gen_strings.py
  se regenerate); mini-card v2 par asal % dikhata hai; "wheel is not
  sending" note sirf tab jab v2 stream na ho (`ShowNoInputNote` ab
  `!HasV2Stream` se gated hai). `docs/UI-LOCKED.md` row 9 ka purana
  handbrake note ("wheel axis nahi bhejta") is faisle se supersede ho
  gaya hai — wo Phase E ka verification record hai, is liye us file ko
  rewrite nahi kiya gaya; ye doc us ki jagah authority hai.
- Default plan mein Handbrake on hai, is liye **"full caps" ka matlab ab
  X + Y + Z + Slider hai** — purane X/Y/Z-only device par Handbrake
  enabled plan validation fail karega aur TryConnect wazeh wajah dega
  (vJoyConf mein Slider enable karo). Ye jaan-boojh kar hai: khamoshi se
  drop karne se behtar hai ke setup ki kami saaf nazar aaye (D11; P2/D9
  ka silent-remap/clamp na karne wala usool).
- Firmware mein koi tabdeeli nahi (slot 4 pehle se emit hota hai).

## Baaqi — Windows PC par verify hoga (Phase H)

Sandbox mein real vJoy driver nahi hai (rig ki maloom had — README
"Real vJoy nahi"), is liye ye do checks Windows 11 PC + vJoy 2.2.2.0 par
honge:

1. `joy.cpl` mein handbrake kheenchne par **Slider** axis 0→100% move kare.
2. Kisi DirectInput sim (e.g. Assetto Corsa) mein handbrake → vJoy Slider
   bind ho aur game mein kaam kare (GAME-COMPATIBILITY.md procedure step 3).
