# RSH — Game Compatibility Matrix (DRAFT)

**Status: UNVERIFIED DRAFT — koi bhi game "works" ke tor par verified nahi hai.**
Ye matrix Phase A (WP5) ka draft hai. Har entry real build par **Phase F** mein verify hogi. Jab tak Phase F ka test na ho, kisi game ke baare mein "verified", "supported" ya "works" ka daawa mat karo — sirf "DRAFT — verify hona hai" likho.
**Source:** Plan v2.3 §5 (Phase F) aur §6 (Industry standard + universal compatibility).

## Phase F — sandbox verification (2026-11-13, W3)

Sandbox (Linux VM, Virtual Test Rig) mein jo verify ho saka, wo yahan hai.
**Koi bhi game is se "VERIFIED" nahi hota** — har game ka status tab hi
badlega jab procedure (§ neeche) real build + Windows 11 PC + asal game par
measure ho. Jo baaqi hai, wo har game ke liye "Baaqi — Windows PC par" hai.

Sandbox mein verify hua:
- **Input chain (code + unit tests):** Wheel → UDP v2.1 → WheelEngine →
  ProcessingPipeline → vJoy (DirectInput) mapping unit tests mein cover hai
  (RSH.VJoy.Tests: layout + validation; RSH.Tests: protocol v2.1, pipeline,
  per-module tuning). Koi bhi game DirectInput device ko is chain se dekhta
  hai — game-side detection sandbox mein mumkin nahi (real vJoy driver +
  game chahiye).
- **Anti-cheat note (code-verified, design ab fact hai):** App game mein
  kuch inject nahi karti — code mein koi injection API nahi hai
  (CreateRemoteThread / WriteProcessMemory / SetWindowsHookEx ka istemal
  kahin nahi; GameLauncher sirf `Process.Start` + `Process.Kill` karta hai).
  Is liye anti-cheat ko aitraaz ki wajah nahi — lekin har game mein
  anti-cheat warning/block ka check phir bhi Windows PC par hoga
  (procedure step 9), kyun ke anti-cheat ka behaviour game + version par
  munhasir hai.
- **Restore-on-exit (A2, SW-14):** GameLauncher.FinalizeExitAsync purani
  profile wapas karta hai (sirf apni launch par, sirf jab user ne beech
  mein profile na badli ho) — sandbox mein stub processes se test hua
  (RSH.App.Tests: restore test + user-choice test). Real game par exit
  flow (launcher + helper process) Windows PC par verify hoga.
- **Stop (SEC-E-10, Phase F fix):** StopAsync ab sirf tracked process
  maarta hai — naam se milne wala doosra same-name instance nahi marta
  (sandbox mein do same-name stub processes se test hua). Assetto Corsa
  helper (acs.exe): launcher ka bacha ho to us ke saath band hota hai,
  warna Refresh dobara attach karke alag se Stop deta hai.
- **Launcher catalogue coverage:** GameLauncher.Catalogue mein 4 games hain
  (Assetto Corsa, ACC, iRacing, Dirt Rally 2.0). Baaki 4 matrix entries
  (rFactor 2, Automobilista 2, F1, Forza) ka launcher mein koi entry nahi —
  un ke Steam/exe paths + launch flow Windows PC par verify karke hi
  catalogue mein add honge (Forza Microsoft Store/Xbox app se aata hai —
  Steam launch path us par laagu nahi hota, is liye us ka rasta alag se
  tay karna hoga).

Sandbox mein verify NAHI ho saka (baaqi — Windows PC par, har game):
procedure ke steps 1–9 (detection, steering, pedals, buttons, encoders,
10-min stability, restore-on-exit real game par, FFB expectation, anti-cheat).

## RSH input chain — ye kaise kaam karta hai (design, verified nahi)

- RSH wheel ESP32 board se Wi-Fi UDP par data bhejta hai; PC app usay **vJoy** (BrunnerInnovation v2.2.2.0) ke zariye **DirectInput** device ke tor par game ko dikhati hai.
- Game Wi-Fi UDP wheel ko kabhi directly nahi dekhta — sirf DirectInput (vJoy) ya XInput device.
- **XInput-only titles vJoy wheel nahi dekhenge** — jo game sirf XInput (Xbox-style controller) input leta hai, us mein RSH wheel nazar nahi aayega.
- **FFB nahi hai:** RSH mein force-feedback motor nahi hai, is liye FFB v2 se bahar hai (extension point: HID PID descriptor, Phase G/H mein evaluate hoga). FFB se mutaliq koi game feature RSH par kaam nahi karega.
- **Rate note:** RSH ka input rate **200 Hz** hai. Ise iRacing ke **360 Hz FFB output** se compare mat karo — ek input hai, doosra FFB output (commercial wheels: Moza 360 Hz FFB output). Game DirectInput device ko apne physics rate (typically 60 Hz) par parhta hai.
- **vJoy keepalive:** Keepalive rehta hai (D32), lekin Phase B mein verify hoga — kuch games idle device ko drop karte hain (unverified).
- **Handbrake mapping (Phase G C8, decided):** handbrake → vJoy **Slider**
  axis (analog). vJoy mein handbrake ka koi dedicated axis nahi — faisla
  aur mustarad options `docs/HANDBRAKE-MAPPING.md` mein hain. Setup: vJoyConf
  mein device 1 par Slider enable karna lazmi hai (USER-GUIDE §2).
- **Anti-cheat note (design, Phase F mein confirm hoga):** App game mein kuch inject nahi karti, is liye anti-cheat ko aitraaz nahi hona chahiye (R-L6). Ye abhi design statement hai, verified fact nahi.
- **Platform:** Windows 11 x64; ARM64 explicitly unsupported (D19). vJoy 2.2.2.0 pin hai; vJoy na mile to app chalegi lekin output disabled rahega aur Dashboard par warning dikhega.

## Matrix

| Game | Input API (expected) | Expected status | Notes |
|---|---|---|---|
| Assetto Corsa | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | DirectInput sim; RSH vJoy device ke tor par nazar aana chahiye — abhi unverified. Launcher catalogue mein hai (AssettoCorsa.exe + acs.exe). |
| Assetto Corsa Competizione (ACC) | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | DirectInput sim; FFB features RSH par kaam nahi karenge (koi motor nahi). Launcher catalogue mein hai (acc.exe). |
| iRacing | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | iRacing ka 360 Hz FFB output hai — RSH ka 200 Hz **input** rate hai, FFB nahi. Dono ko compare mat karo. FFB RSH mein nahi hai. Launcher catalogue mein hai (iRacingSim64DX11.exe). |
| rFactor 2 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | DirectInput sim; FFB features RSH par kaam nahi karenge. **Launcher catalogue mein NAHI** — exe/Steam path Windows PC par verify karke add hoga. |
| Automobilista 2 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | DirectInput sim; FFB features RSH par kaam nahi karenge. **Launcher catalogue mein NAHI** — exe/Steam path Windows PC par verify karke add hoga. |
| Dirt Rally 2.0 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | DirectInput sim; FFB features RSH par kaam nahi karenge. Launcher catalogue mein hai (dirt2.exe). |
| F1 (series) | DirectInput (vJoy) — expected | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | Series mein titles alag-alag hain; har title alag se verify hoga. FFB features RSH par kaam nahi karenge. **Launcher catalogue mein NAHI** — har title ka exe alag se verify karke add hoga. |
| Forza (series) | DirectInput (vJoy) — expected; kuch titles XInput-only ho sakte hain | **DRAFT — Phase F mein verify hoga (baaqi: Windows PC par)** | Series mein titles alag-alag hain; jo title XInput-only ho, us mein vJoy wheel nazar nahi aayega. Har title alag se verify hoga. **Launcher catalogue mein NAHI** — Forza Microsoft Store/Xbox app se aata hai, Steam launch path laagu nahi hota. |

**Games ki tadaad:** 8 entries (F1 aur Forza series ke tor par — har title alag se verify hoga).

## Verification procedure (Phase F)

Har listed game par real build (firmware v4.0.0 + app v2.0.0, Windows 11 x64, vJoy 2.2.2.0) se ye test hoga:

1. **Detection:** Game launch karo — RSH wheel DirectInput device ke tor par game ki controls/settings screen mein nazar aata hai ya nahi? (XInput-only title ho to "not detected — XInput-only" record karo, bug nahi.)
2. **Steering:** Steering axis full lock-to-lock move karo — game mein steering sahi direction aur range mein respond karta hai? (D7 scale divide ke saath.)
3. **Pedals:** Accelerator, brake, clutch (aur handbrake, jahan configured ho) — har axis 0–100% sahi respond karta hai?
4. **Buttons:** Kam az kam configured buttons (D1 u32 encoding, D9 >32 reject rule) game mein bind hote hain aur kaam karte hain?
5. **Encoders:** Configured encoders (jahan hon) game mein input dete hain?
6. **Stability / idle:** 10-minute session — device drop to nahi hota? (D32 keepalive check; agar game idle device drop kare to record karo.)
7. **Restore-on-exit (A2):** Game band karne par app vJoy device ko restore karti hai? (Phase F exit gate.)
8. **FFB expectation check:** FFB kaam nahi karega — ye expected hai (koi motor nahi); FFB ki kami ko failure mat likho, "expected — no FFB" record karo.
9. **Anti-cheat:** Jahan game mein anti-cheat ho — launch + session mein koi anti-cheat warning/block to nahi aata? (R-L6 design note confirm karo.)

**Result recording:** Har game ke liye result is matrix mein update hoga — sirf Phase F ke measured test ke baad status "DRAFT" se "VERIFIED (Phase F, date)" mein badlega. Unverified entries ko kabhi "works" mat likho.
