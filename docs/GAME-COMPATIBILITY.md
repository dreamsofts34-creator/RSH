# RSH — Game Compatibility Matrix (DRAFT)

**Status: UNVERIFIED DRAFT — koi bhi game "works" ke tor par verified nahi hai.**
Ye matrix Phase A (WP5) ka draft hai. Har entry real build par **Phase F** mein verify hogi. Jab tak Phase F ka test na ho, kisi game ke baare mein "verified", "supported" ya "works" ka daawa mat karo — sirf "DRAFT — verify hona hai" likho.
**Source:** Plan v2.3 §5 (Phase F) aur §6 (Industry standard + universal compatibility).

## RSH input chain — ye kaise kaam karta hai (design, verified nahi)

- RSH wheel ESP32 board se Wi-Fi UDP par data bhejta hai; PC app usay **vJoy** (BrunnerInnovation v2.2.2.0) ke zariye **DirectInput** device ke tor par game ko dikhati hai.
- Game Wi-Fi UDP wheel ko kabhi directly nahi dekhta — sirf DirectInput (vJoy) ya XInput device.
- **XInput-only titles vJoy wheel nahi dekhenge** — jo game sirf XInput (Xbox-style controller) input leta hai, us mein RSH wheel nazar nahi aayega.
- **FFB nahi hai:** RSH mein force-feedback motor nahi hai, is liye FFB v2 se bahar hai (extension point: HID PID descriptor, Phase G/H mein evaluate hoga). FFB se mutaliq koi game feature RSH par kaam nahi karega.
- **Rate note:** RSH ka input rate **200 Hz** hai. Ise iRacing ke **360 Hz FFB output** se compare mat karo — ek input hai, doosra FFB output (commercial wheels: Moza 360 Hz FFB output). Game DirectInput device ko apne physics rate (typically 60 Hz) par parhta hai.
- **vJoy keepalive:** Keepalive rehta hai (D32), lekin Phase B mein verify hoga — kuch games idle device ko drop karte hain (unverified).
- **Anti-cheat note (design, Phase F mein confirm hoga):** App game mein kuch inject nahi karti, is liye anti-cheat ko aitraaz nahi hona chahiye (R-L6). Ye abhi design statement hai, verified fact nahi.
- **Platform:** Windows 11 x64; ARM64 explicitly unsupported (D19). vJoy 2.2.2.0 pin hai; vJoy na mile to app chalegi lekin output disabled rahega aur Dashboard par warning dikhega.

## Matrix

| Game | Input API (expected) | Expected status | Notes |
|---|---|---|---|
| Assetto Corsa | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | DirectInput sim; RSH vJoy device ke tor par nazar aana chahiye — abhi unverified. |
| Assetto Corsa Competizione (ACC) | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | DirectInput sim; FFB features RSH par kaam nahi karenge (koi motor nahi). |
| iRacing | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | iRacing ka 360 Hz FFB output hai — RSH ka 200 Hz **input** rate hai, FFB nahi. Dono ko compare mat karo. FFB RSH mein nahi hai. |
| rFactor 2 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | DirectInput sim; FFB features RSH par kaam nahi karenge. |
| Automobilista 2 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | DirectInput sim; FFB features RSH par kaam nahi karenge. |
| Dirt Rally 2.0 | DirectInput (vJoy) | **DRAFT — Phase F mein verify hoga** | DirectInput sim; FFB features RSH par kaam nahi karenge. |
| F1 (series) | DirectInput (vJoy) — expected | **DRAFT — Phase F mein verify hoga** | Series mein titles alag-alag hain; har title alag se verify hoga. FFB features RSH par kaam nahi karenge. |
| Forza (series) | DirectInput (vJoy) — expected; kuch titles XInput-only ho sakte hain | **DRAFT — Phase F mein verify hoga** | Series mein titles alag-alag hain; jo title XInput-only ho, us mein vJoy wheel nazar nahi aayega. Har title alag se verify hoga. |

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
