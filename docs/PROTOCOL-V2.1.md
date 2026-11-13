# RSH Protocol v2.1 — byte-level contract (WP3, Phase A)

Status: **CONTRACT — Phase A WP3 deliverable. Authority for all v2.1 wire
formats.** Plan v2.3 §1 ke faisle (D1–D6, D27–D30) yahan byte-level likhe gaye
hain. Jahan is doc aur purani `PROTOCOL-V2.md` (v2 draft) mein farq ho, **ye
doc jeet-ti hai**. Reference encoder + golden vectors:
`tests-host/protocol_v2_vectors.py` (run it). Pin rules:
`docs/board-esp32-devkitv1.json` + `tests-host/pin_rules_ref.py`. Key lifecycle:
`docs/KEY-LIFECYCLE.md`.

Versions: firmware **v4.0.0**, app **v2.0.0**, protocol **v2.1** (D27).
Product naam: **RSH**.

## 0. Conventions (sab packets/frames par laagu)

- Sab integers **little-endian**, jab tak explicitly big-endian na likha ho.
- **CRC16 = CRC-16/CCITT-FALSE**: poly `0x1021`, init `0xFFFF`, refin/out
  false, xorout `0x0000`. Check value: `CRC16("123456789") = 0x29B1`.
  CRC hamesha **packet/frame ke aakhir mein** hota hai aur CRC field se pehle
  ke **tamam bytes** par banta hai (D2). Koi packet/frame CRC ke baad extra
  bytes nahi rakhta.
- **UDP ports** (unchanged): PC→board `4210` (HELO), board→PC `4211`
  (stream, status, HELLO reply, PIN_PROBE frames), control dono taraf `4212`.
  Ping (§9) bhi `4212` par hai — control se magic se demux hota hai
  (`"RSHN"` vs `"RSHC"`), is liye ping ke liye naya port / firewall rule nahi.
- **Reserved bytes hamesha 0** bhejta hai; receiver nonzero reserved par
  packet/frame **drop + count** karta hai (D6).
- **Bounds (D6):** nAxes ≤ 8, nBtn ≤ 128 (config ceiling), nEnc ≤ 4. Is se
  zyada claim karne wala stream frame **BAD_FRAME — reject (drop + count)**.
  Wire par buttons 0–63 aate hain (neeche §1.1/§1.2); nBtn 65–128 ki config
  SET_CFG par `UNSUPPORTED` se reject hoti hai (dekho §1.1 note N1).
- **mvMask khatam (D6):** sab axes millivolt (mV) mein hain. Koi per-axis
  unit flag nahi hai. Axis value u16 mV hai (0–3300 typical; ADC raw kabhi
  wire par nahi aata).
- **Version byte:** v2.1 wire version byte = `0x21` (major 2, minor 1, ek
  byte mein `major<<4 | minor`). Legacy v1 packet pehchan: pehla byte `0xA5`
  aur doosra byte `0x01` → legacy mode foran (R-H18). Path selection **board
  version** se hoti hai (HELLO reply §2 / GET_CAPS §3 se), profile version
  se nahi.
- **Upgrade order (R-M10):** pehle app, phir firmware. App v2 purane board
  se legacy mode mein chalti hai. OTA ke baad app GET_CAPS se firmware
  version verify karegi; version < 4.0.0 ho to v2 features disable rahenge.
  Ulta order support nahi.
- **No HELO for 2000 ms ⇒ board goes silent** (unchanged, v1 jaisa).

## 1. Board → PC stream

### 1.1 Data packet v2.1 (port 4211) — variable length

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSH2"` | ASCII `0x52 0x53 0x48 0x32` |
| 4 | version | u8 | `0x21` (= v2.1). `!= 0x21` → drop + count |
| 5 | flags | u8 | §1.3 flags table (D4) |
| 6–7 | seq | u16 | Stream sequence, har packet +1, wrap `0xFFFF → 0` |
| 8–11 | timestamp | u32 | µs, **ADC read ke baad** liya gaya (F4/D13) |
| 12 | axisMask | u8 | bit *n* = axis *n* present, n = 0..7 |
| 13 | nEnc | u8 | Encoder count, 0..4. `> 4` → BAD_FRAME drop |
| 14–17 | buttons | u32 | bit *n* = button *n*, n = 0..31 (D1) |
| 18–19 | reserved | u16 | Hamesha `0`. Nonzero → drop + count (D6) |
| 20… | axes | u16 × popcount(axisMask) | Ascending axis index, **mV** |
| … | encoders | i16 × nEnc | **Cumulative** counts (D1/D6 note), per encoder |
| last 2 | CRC16 | u16 | §0 ke mutabiq |

Length = `20 + 2·popcount(axisMask) + 2·nEnc + 2`.
`popcount(axisMask) > 8` mumkin nahi (mask 8 bit hai). Axis indices **slots**
hain, roles (steering, throttle…) config se map hote hain — PC mapping
`GET_CFG`/`GET_CAPS` se seekhta hai, hard-coded positions se nahi.

> **Canonical default slot map (Phase F WS5, 2026-11-13):** factory blob
> mein axes ye slots lete hain — 0 Steering, 1 Accelerator, 2 Brake,
> 3 Clutch, 4 Handbrake. Clutch default OFF hai (pin GPIO33, `PIN_CLUTCH_ADC`);
> enabled tab hoti hai jab config blob mein `AXIS_ADC` slot 3, pin 33 ho.
> Handbrake slot 3 se slot 4 par muntaqil hua (Phase E WS5 stream-emission
> contract se tabdeeli — host tests §23 isi map par hain).
>
> **H-pattern gear bits (Phase F WS5):** Shifter gate mode H-pattern ho
> to gears 1–6 stream `buttons` bits **5–10** par aate hain (bit 5 = gear 1
> … bit 10 = gear 6; gear-up/down bits 0–4 se takrao nahi — D1 absolute
> numbering barkarar). Core `Gear` decode karta hai (0 = neutral; exactly
> ek gear bit set ho warna 0) aur gear bits ko `ButtonsMask` se hata deta
> hai. Sequential mode mein bits aam buttons ki tarah guzarte hain aur
> `Gear` hamesha 0. Gate mode SET_CFG type-11 `SHIFTER` record se aata hai
> (§3.6).

**Encoders cumulative (D1):** har encoder ka value boot se cumulative count
hai (i16, wrap allowed — PC delta `new - old` ko i16 wrap ke saath nikale).
v2 draft ke i8 detents **khatam**.

**Note N1 — buttons 64–127 (D1 vs D6 tension, coordinator review):**
D1 kehta hai buttons 33+ status packet mein `btnsEx u32` se aate hain, aur D6
kehta hai nBtn ≤ 128. Ek u32 sirf 32 buttons cover karta hai, is liye v2.1
wire par **buttons 0–63** aate hain: stream `buttons` (0–31) + status
`btnsEx` (32–63, §1.2). Config mein nBtn 65–128 D6 ceiling ke andar hai
lekin us ka wire slot v2.1 mein nahi hai — SET_CFG aisi config ko
`UNSUPPORTED` (6) se reject karta hai. v2.2 mein doosra extension word
define ho sakta hai. Parser bound ke tor par nBtn > 128 hamesha BAD_FRAME
hai. Board JSON (`board-esp32-devkitv1.json`) ka `limits.buttons = 32` v3.3
pin map ki limit hai, wire limit nahi.

### 1.2 Status packet v2.1 (port 4211, 1 Hz) — 34 B base, 38 B extended

Base layout (nBtn ≤ 32, koi `btnsEx` nahi):

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0 | magic | u8 | `0xA6` (data packet se alag) |
| 1 | version | u8 | `0x21` |
| 2–3 | seq | u16 | Status sequence (data seq se alag counter) |
| 4–7 | uptime | u32 | ms since boot |
| 8 | rssi | i8 | dBm (PC link ka) |
| 9 | battery | u8 | %; `255` = no battery module |
| 10–11 | supply | u16 | mV; `0` = not measured |
| 12–15 | heap | u32 | Free heap bytes |
| 16–17 | overruns | u16 | Since boot |
| 18–19 | maxCycle | u16 | Max cycle µs (poori loop iteration, D15) |
| 20 | governor | u8 | Governor level |
| 21 | resetReason | u8 | 1 power-on, 2 brown-out, 3 watchdog, 4 software, baaqi reserved |
| 22–25 | cfgCrc | u32 | Config CRC32 |
| 26–27 | cfgVer | u16 | Config version |
| 28–29 | seqGap | u16 | **NEW (D5)** — boot se skip hue stream sequence numbers ki ginti (tick-skipped events). Wrap allowed |
| 30–31 | txFail | u16 | **NEW (D5/F3)** — boot se failed UDP transmissions ki ginti. Wrap allowed |
| 32–33 | CRC16 | u16 | §0 ke mutabiq, **aakhir mein** |

Total **34 B** (D5).

**Coordinator-review note (offsets):** Plan v2.3 D5 ke parenthetical offsets
(`seqGap (30–31) + txFail (32–33)`) CRC ko purani jagah (28–29) par rakhte
hain — us se CRC packet ke beech mein aa jata aur total 34 B ke saath CRC
"aakhir mein" na rehta. v2.1 CRC-last invariant (§0, D2) ko tarjeeh deta hai:
`seqGap` 28–29, `txFail` 30–31, CRC 32–33. Agar coordinator purane offsets
chahta hai to D5 ko 36 B karna hoga — faisla review mein confirm ho.

**Extended layout (nBtn 33–64):** jab board par 33–64 buttons configured
hain, status packet **38 B** ka hota hai — UDP datagram length se pehchana
jata hai (34 = base, 38 = extended; aur koi length valid nahi):

| Bytes | Field |
|---|---|
| 0–27 | Base fields (upar jaisi) |
| 28–31 | `btnsEx` u32 — bit *(n−32)* = button *n*, n = 32..63 (D1) |
| 32–33 | seqGap u16 |
| 34–35 | txFail u16 |
| 36–37 | CRC16 |

**Wi-Fi channel status mein nahi aata** — wo GET_CAPS reply mein hai (D5).

### 1.3 Flags byte (data packet byte 5) — D4

| Bit | Naam | Matlab (1 = true) |
|---|---|---|
| 0 | config-changed | Board ki config PC ke paas wali se badal gayi — PC `GET_CFG` kare |
| 1 | low-supply | Supply voltage low (threshold Phase D, F2/F10) |
| 2 | tick-skipped | Is packet se pehle ek ya zyada ticks skip hue (flash commit jaisi wajah) |
| 3 | axis-invalid | Koi axis invalid hai (F2 criteria, D12 — sirf §0.1 note dekho) |
| 4–7 | reserved | Hamesha `0` |

v2 draft ka bit0 "steering in mV" **khatam** — sab axes mV hain (D6).
Axis-invalid criteria (D12): invalid sirf tab jab (a) value 10 s tak exact
rail (0 ya 4095) par stuck ho **aur** doosre axes move kar rahe hon, ya (b)
variance threshold se zyada ho (floating noise). Sirf variance≈0 kabhi
invalid ki wajah nahi. PIN_PROBE se confirm karo.

### 1.4 Stream parser reject rules (PC side, T3)

Drop + count karo (counter alag-alag: `badMagic`, `badVersion`, `badCrc`,
`badFrame`, `rsvNonzero`):

- Pehle 4 bytes `!= "RSH2"` aur pehle 2 bytes `!= 0xA5 0x01` → `badMagic`.
- Version byte `!= 0x21` → `badVersion`.
- `nEnc > 4` ya datagram length != `20 + 2·axes + 2·nEnc + 2` → `badFrame`.
- Reserved bytes 18–19 `!= 0` → `rsvNonzero` (D6).
- CRC16 mismatch → `badCrc`.
- Flags bits 4–7 nonzero → drop + count (reserved violation).

## 2. HELLO / pairing (HELO + HELLO reply)

### 2.1 HELO request (PC → board, port 4210)

| Bytes | Field | Notes |
|---|---|---|
| 0–3 | `"HELO"` | ASCII |
| 4 | proto | u8: `0x01` = v1 app (legacy, koi reply nahi — v1 18 B packets); `0x02` = v2.x app |
| 5 | sessionFlag | u8, sirf proto `0x02` ke saath: `0` = resume (purana nonce wapas), `1` = naya session (naya nonce, counter 0 se). 5-byte HELO (sessionFlag ghair-maujood) = `0` mana jata hai (v2 draft compat) |

App routine HELO **1 Hz** bhejti hai (sessionFlag `0`); board har HELO ka
jawab HELLO reply se deta hai. 2000 ms tak koi HELO na aaye to board silent
ho jata hai.

### 2.2 HELLO reply (board → PC, port 4211, sender ke source port par) — 92 B fixed

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHR"` | ASCII `0x52 0x53 0x48 0x52` |
| 4 | protoMajor | u8 | `2` |
| 5 | protoMinor | u8 | `1` |
| 6 | flags | u8 | bit0 `unpaired` (1 = unpaired, key+password fields valid); bit1 `keyResetArmed`; bit2 `safeMode`; bit3 `pairingWindowOpen`; bit4–7 reserved `0` |
| 7 | sessionEcho | u8 | Request ka sessionFlag (`0`/`1`) |
| 8–13 | boardMac | 6 B | Board Wi-Fi MAC (device identity; **key/password is se derive kabhi nahi** — R-H10) |
| 14 | fwMajor | u8 | `4` |
| 15 | fwMinor | u8 | `0` |
| 16 | fwPatch | u8 | `0` |
| 17 | reserved | u8 | `0` |
| 18–33 | nonce | 16 B | §2.3 nonce rules |
| 34–65 | deviceKey | 32 B | **Sirf** unpaired + pairing window ke andar: per-device key (OPEN-2). Paired ho ya window band ho to **sab `0x00`** |
| 66–89 | wifiPassword | 24 B | **Sirf** unpaired + window ke andar: per-device Wi-Fi/OTA password, ASCII (§5.1). Paired/window-band ho to **sab `0x00`** |
| 90–91 | CRC16 | u16 | §0 ke mutabiq |

Total **92 B fixed** — parser length se validate kare; `!= 92` → drop.

### 2.3 Nonce / counter / session rules (D3, D30)

- **Nonce:** 16 bytes, hardware RNG se. **Sirf tab rotate** hota hai jab
  board reboot ho ya PC naya session maange (HELO sessionFlag `1`).
  Routine 1 Hz HELO (sessionFlag `0`) **purana nonce** wapas deta hai.
- **Counter:** u32, **per-session**. Naya nonce = counter `0` se shuru.
  Session ke andar har **naye** control request par counter +1 (retry par
  bhi naya counter — §3.2). Counter LE u32 ke tor par HMAC input mein jata
  hai.
- **Session khatam:** naya nonce milne par purana nonce/counter dobara
  istemal nahi hota. Purane nonce se signed frame → `BAD_TAG`.
- **Pairing window (OPEN-5 — CLOSED):** window **SoftAP up hone se 60 s**
  tak khulti hai. Window khatam hone ki nishani: HELLO reply flags bit3
  `pairingWindowOpen = 0` aur `deviceKey`/`wifiPassword` fields zero. Window
  ke baad dobara pairing ke liye KEY_RESET (§4.4) lazmi hai.

### 2.4 HELO acceptance (Phase D, SEC-D-08)

Pehle 4 bytes `"HELO"` dekhte hi `pcIp` overwrite karna hijack tha — AP
par koi bhi station ek packet se 200 Hz stream apni taraf mor sakta tha.
Ab board ye rules lagata hai:

- **Proto check:** 5th byte `0x01` (legacy) ya `0x02` (v2) hona lazmi hai;
  kuch aur ho to HELO ignore. `sessionFlag` (6th byte) `0`/`1` ke ilawa
  ho to bhi ignore.
- **Live-session takeover mana:** jab tak koi PC active ho (aakhri HELO
  2 s ke andar), koi **doosra IP** `pcIp` overwrite nahi kar sakta — us
  ka HELO ignore hota hai (na timer refresh, na HELLO reply).
- **Flap throttle:** idle board par bhi `pcIp` change 5 s mein ek se
  zyada nahi hota.
- **HELLO reply rate limit:** 92 B reply vs 5 B request = 18x
  amplification; flood par board reflector na bane — 500 ms mein ek se
  zyada HELLO reply nahi.
- **Residual (documented):** idle board (koi active PC nahi) par koi bhi
  HELO bhej kar stream le sakta hai — unauthenticated UDP bootstrap ki
  fitri had hai (OPEN-6). Pairing window (60 s) + pairing ke baad WPA2
  is ki had bandhte hain; mukammal hal DTLS hai (deferred, §0.8).
- **SEC-E-14 (Phase F):** stream emission ab authenticated session se
  bandhi hai — board 200 Hz stream (v1 + v2) sirf tab bhejta hai jab is
  session mein kam az kam ek HMAC-verified control command ACCEPT hua
  ho (aam tor par GET_CAPS; `stream_emit.h` / `net.cpp`
  `sessionAuthenticated`). HELO akela stream nahi kholta; naya peer ya
  naya nonce (sessionFlag=1) flag saaf kar deta hai. Nateeja: app ko
  har (dobara) connect par pehle ek signed control command (GET_CAPS)
  bhejna lazmi hai — sirf HELO bhejne wala dashboard khaali rehta hai.
  1 Hz status packet is gate se bahar hai (diagnostics, stream nahi).

## 3. Control frames (port 4212, dono taraf)

### 3.1 Frame layout

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHC"` | ASCII `0x52 0x53 0x48 0x43` |
| 4 | cmd | u8 | §3.3 table; reply mein `reqCmd \| 0x80` |
| 5–6 | reqId | u16 | PC chunta hai; retry mein **wahi** reqId (D3) |
| 7–10 | counter | u32 | Per-session monotonic (§2.3, §3.2) |
| 11–12 | len | u16 | Payload length, 0..1024. `> 1024` → `BAD_LENGTH` (parse result); reply status `TOO_BIG` (4) |
| 13… | payload | bytes | `len` bytes |
| last 16 | tag | 16 B | HMAC-SHA256 (§3.1) ke pehle 16 bytes |

Total = `13 + len + 16`. **Max frame 1053 B.**

**Parse error codes (structure parse, §3.2):** `TOO_SHORT` sirf tab jab
frame **13 B se chhota** ho (header ka `len` field hi parha na ja sake).
Header parh liya gaya ho to har length mismatch → `BAD_LENGTH` — `len >
1024` ho, ya total length `13 + len + 16` se match na kare, **truncation
samet** (misal 29 B ka GET_CAPS frame 8 B truncate ho kar 21 B reh jaye
to `BAD_LENGTH`, `TOO_SHORT` nahi). C# (`ControlFrame.TryParseStructure`)
aur C++ (`rsh_v2_ctrl_parse_structure`) dono mein yahi rule hai.

### 3.2 HMAC + duplicate / replay rules (D3)

- **Key:** per-device random **32 bytes** (OPEN-2 — CLOSED). Koi password
  hash nahi (OPEN-1 — dekho §6.1). Key board pehle boot par hardware RNG se
  banata hai, NVS mein rehti hai (plaintext — encrypted NVS abhi OPEN-3 hai,
  Phase H; `docs/KEY-LIFECYCLE.md` §2 dekho); app DPAPI se store karti hai
  (`docs/KEY-LIFECYCLE.md` §4).
- **Tag:** `tag = HMAC-SHA256(key, nonce ‖ counter_LE ‖ cmd ‖ payload)[0:16]`
  — nonce 16 B (HELLO reply se), counter u32 LE, cmd 1 B, payload raw.
  **Dhyaan:** frame header (magic/reqId/len) HMAC mein **nahi** hai; sirf
  nonce, counter, cmd, payload hain.
- **Reply tag:** wahi formula, reply ke `cmd` (req|0x80) aur reply payload
  ke saath, request wala counter echo karke. PC reply ka tag verify kare;
  fail ho to reply drop kare aur request ko retry kare (naya counter).
- **Retry:** PC ek request **3×** tak resend karta hai (250 ms interval),
  har retry mein **reqId wahi, counter naya**. Board har retry ko **nayi
  execution** samajhta hai — is liye tamam commands **idempotent** honi
  chahiye (SET_CFG chunk dobara likhna, COMMIT/CONFIRM dobara — sab safe).
- **Duplicate (exact retransmit):** board pichle **32** `(reqId, counter)`
  pairs (per session) cache rakhta hai. Wahi pair dobara aaye to board
  **cached reply dobara bhejta hai, dobara execute nahi karta**. Reply
  cache bhi 32 slots hai; sirf ≤ 128 B replies cache hote hain — is se
  bara reply (misal poora pin table) cache nahi hota, us ka duplicate
  dobara execute hota hai (tamam commands idempotent honi chahiye, is
  liye ye fallback mehfooz hai).
- **Replay:** naya reqId lekin counter session ke max-seen se **purana**
  ho → reply status `REPLAY` (2). "Purana" ka matlab RFC 1982 serial
  arithmetic hai: `(int32_t)(counter − max_seen) ≤ 0` → replay. Is se
  **counter wrap** bhi sahi chalta hai: `0xFFFFFFFF` ke baad `0` naya
  counter hai, purana nahi. Board koi bhi counter gap accept karta hai
  (counter > max-seen theek hai — khoya packet session ko wedge nahi karta).
- **Malformed frame** (bad magic, `len` datagram se match na kare, truncated)
  → **bina reply ke drop** (reqId par bharosa nahi kiya ja sakta). Tag fail
  hone par (frame parse ho gaya ho) → reply status `BAD_TAG` (1) — reply
  mein bhi tag hota hai (request ke counter/cmd se bana hua nahi; board
  apna tag `BAD_TAG` reply par usi formula se banata hai, lekin PC is reply
  ka tag verify **nahi** kar sakta jab key mismatch ho — is liye PC `BAD_TAG`
  reply ko sirf tab maanta hai jab reqId/counter match karein).
- **Timeout:** PC 250 ms mein reply na aaye to retry; 3 retries ke baad
  command fail (UI ko dikhaye). GET_CAPS timeout **2 s** sirf legacy
  fallback ke liye hai (R-H18) — normal command timeout 250 ms × 3 hai.

**Header malleability (SEC-D-07a — Phase D amendment, D3):** tag mein
sirf `nonce ‖ counter ‖ cmd ‖ payload` hai — header (`magic`/`reqId`/
`len`) HMAC mein **nahi** hai. Is ka asar tajziya kiya gaya hai:
`reqId` badalne se koi **nayi execution nahi** hoti — badla hua `reqId`
ya to cached pair se takra kar **DUPLICATE** (cached reply, no execute)
banta hai, ya naye `reqId` + purane counter par **REPLAY** (status 2, no
execute) banta hai. `len` badalne se total length `13 + len + 16` se
match nahi karti → **BAD_LENGTH, silent drop**. Payload/cmd/counter
badle baghair koi doosra command execute nahi ho sakta. Is liye tag
formula **v2.1-frozen rehta hai** — `reqId`/`len` ko HMAC mein shamil
karna tamam 3-language golden vectors (C#/C++/Python) + deployed app
v2.0.0 ko invalid kar deta, jab ke security faida sifar hai (upar wala
tajziya). Ye faisla yahan darj hai taake khamoshi se na badle.

### 3.3 Commands

| cmd | Name | Request payload → Reply payload (`status u8` pehle) |
|---|---|---|
| `0x01` | GET_CAPS | — → §3.4 GET_CAPS body |
| `0x02` | GET_CFG | `offset u16, wantLen u16 (≤1024)` → `status, totalLen u16, offset u16, data…` |
| `0x03` | SET_CFG | §3.5 chunk → `status, stagedBytes u16, totalExpected u16` |
| `0x04` | COMMIT | — → `status, confirmDeadlineSec u8 (= 30)`; §3.6 |
| `0x05` | CONFIRM | — → `status` |
| `0x06` | REVERT | — → `status` |
| `0x07` | PIN_PROBE | `pin u8, durationMs u16, mode u8` → `status, acceptedMs u16`; phir §3.8 stream frames |
| `0x08` | FACTORY_RESET | — → `status`; phir reboot (key+password **rehte** hain, §4.3) |
| `0x09` | REBOOT | — → `status`; phir reboot. **Waahid restart rasta hai**
  (SEC-D-01): unauthenticated `"RSTX"` (port 4210) **hata diya gaya** —
  restart sirf HMAC-signed REBOOT se hota hai, reply bhejne ke baad. |
| `0x0A` | LED_TEST | §3.7 pattern → `status` |
| `0x0B` | KEY_RESET | — → `status`; sirf key-reset-armed state mein (§4.4, OPEN-10) |
| `0x0C` | SET_PASSWORD | `newLen u8 (8..63), password[newLen] ASCII` → `status`; §5.2 |

Unknown cmd → reply status `UNSUPPORTED` (6), cmd echo `req|0x80`.
Busy (flash commit chal raha ho, confirm window mein conflicting op) →
`BUSY` (7).

### 3.4 GET_CAPS reply body (status ke baad)

| Bytes (relative) | Field | Type | Notes |
|---|---|---|---|
| 0 | status | u8 | §3.9 |
| 1–6 | boardMac | 6 B | = HELLO reply boardMac |
| 7 | fwMajor | u8 | `4` |
| 8 | fwMinor | u8 | `0` |
| 9 | fwPatch | u8 | `0` |
| 10 | protoMajor | u8 | `2` |
| 11 | protoMinor | u8 | `1` |
| 12 | nAxesMax | u8 | `8` |
| 13 | nBtnMax | u8 | `64` (wire; config ceiling 128, §1.1 N1) |
| 14 | nEncMax | u8 | `4` |
| 15 | wifiChannel | u8 | Current SoftAP channel (1–13); `0` = unknown / AP nahi |
| 16 | capsFlags | u8 | bit0 `slowAxis` (ADS1115 axis maujood, ≤50 Hz); bit1 `safeMode`; bit2 `unpaired`; bit3 `keyResetArmed`; bit4–7 reserved `0` |
| 17 | pinCount | u8 | N — pin table entries (0..40) |
| 18… | pin table | 8 B × N | Har entry §3.4.1 |

Pin table entry (8 B):

| Bytes | Field | Notes |
|---|---|---|
| 0 | pin | GPIO number (0–39) |
| 1 | kind | §3.4.1 kind codes |
| 2 | module | §3.4.1 module codes (0 = NONE / unassigned) |
| 3 | caps | bit0 input, bit1 output, bit2 adc, bit3 pwm, bit4 pullup, bit5–7 reserved `0` |
| 4–7 | reserved | `0` |

#### 3.4.1 Kind / module codes

Kind: `0 NONE · 1 ADC_NATIVE · 2 ADC_ADS1115 · 3 DIGITAL_IN ·
4 DIGITAL_OUT · 5 LED_STRIP · 6 I2C_SDA · 7 I2C_SCL ·
8 EXPANDER_MCP23017 · 9 EXPANDER_PCF8574 · 10 AXIS_GENERIC ·
11 RESERVED_PIN · 12 AXIS_AS5600` (Phase G WS4 — magnetic encoder axis,
I²C addr 0x36 fixed, max 1/board; `docs/AS5600-SPEC.md`, D34).
`RESERVED_PIN` strapping/forbidden hai — `docs/board-esp32-devkitv1.json`
`class` forbidden/avoid/reserved se map hota hai; WP1/WP2 pin rules hi
authority hain — GET_CAPS sirf board ki current table batata hai).

Module (roles, Plan v2.3 §4): `0 NONE · 1 STEERING · 2 ACCELERATOR ·
3 BRAKE · 4 CLUTCH · 5 HANDBRAKE · 6 SHIFTER · 7 HORN · 8 BUTTONS ·
9 ENCODERS · 10 LED · 11 BATTERY`.

### 3.5 SET_CFG chunk layout (request payload)

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–1 | offset | u16 | Is chunk ka config blob mein offset (0-based) |
| 2–3 | total | u16 | Poori config blob ki length (≤ 2048). Har chunk mein **wahi** total |
| 4–5 | chunkLen | u16 | Is chunk mein data bytes (1..1024) |
| 6–7 | chunkCrc | u16 | CRC16 (§0) sirf `data` par |
| 8… | data | bytes | `chunkLen` bytes |

Rules: chunk ≤ 1024 B, total ≤ 2048 B (2 KB). Chunk `offset + chunkLen`
`total` se zyada ho → `TOO_BIG` (4). Chunk CRC mismatch → `BAD_CRC` (3).
Pehla chunk offset 0 ka hona chahiye (wo session kholta hai — kisi aur
offset se shuru ho to `TOO_BIG`); us ke baad chunks kisi bhi order mein
aa sakte hain; board staged buffer mein likhta hai; poora blob milne par
(staged == total) board blob CRC32 (trailer, §3.6) verify karta hai — fail
ho to agle COMMIT par `PIN_CONFLICT` nahi, `BAD_CRC` (3) milega (COMMIT
reply status byte mein). Naya SET_CFG session (offset 0, alag total) purana
staged buffer discard karta hai.

### 3.6 Config blob (staged / stored, §3.5 ka `data`)

Config blob magic **`"RSHG"`** hai (v2 draft ka `"RSHC"` **badal diya gaya**
— `"RSHC"` control frame magic se takrao khatam karne ke liye; blob sirf
SET_CFG payload ke andar aata hai, wire frame nahi hai).

```
Header (8 B): 'RSHG' | schema u8 (= 2) | moduleCount u8 | cfgVersion u16 LE (offset 6)
Module record (fixed 16 B): type u8 | slot u8 | flags u8 | pinA u8 | pinB u8 | pinC u8 | params[10]
Trailer (4 B): CRC32 (header + records par, CRC-32/ISO-HDLC, poly 0xEDB88320, init 0xFFFFFFFF)
```

Types: `1 AXIS_ADC · 2 AXIS_ADS1115 · 3 BUTTON · 4 BUTTON_MATRIX ·
5 ENCODER · 6 LED_STRIP · 7 BATTERY · 8 EXPANDER_MCP23017 ·
9 EXPANDER_PCF8574 · 10 AXIS_GENERIC · 11 SHIFTER` (Phase F WS5) ·
`12 AXIS_AS5600` (Phase G WS4 — magnetic encoder axis, I²C addr 0x36
fixed, max 1/board; record layout `docs/AS5600-SPEC.md` §5).
Slot semantics (board-side — type/slot hi role tay karta hai; blob hi
poori sach hai — **replace semantics**: jis function ka record blob mein
nahi, board usay sample hi nahi karta, pin `0xFF`):

| Type | Slot | Role (board) |
|---|---|---|
| `1 AXIS_ADC` | 0 | Steering |
| `1 AXIS_ADC` | 1 | Accelerator |
| `1 AXIS_ADC` | 2 | Brake |
| `1 AXIS_ADC` | 3 | Clutch (koi bhi ADC1 pin 32/33/34/35/36/39) |
| `1 AXIS_ADC` | 4 | Handbrake |
| `7 BATTERY` | 0 | Battery sense |
| `3 BUTTON` | 0 | Shifter up (sequential) |
| `3 BUTTON` | 1 | Shifter down (sequential) |
| `3 BUTTON` | 2 | Horn |
| `5 ENCODER` | 0 | Encoder 1 (pinA/pinB) |
| `5 ENCODER` | 1 | Encoder 2 (pinA/pinB) |
| `6 LED_STRIP` | 0 | LED strip data |
| `11 SHIFTER` | — | Gate mode + H-pattern gear pins (neeche) |

Factory default blob (Phase G C3, 188 B, 11 records, cfgVersion 1):
AXIS 0:34, 1:35, 2:36, 4:32 · BATTERY 0:39 · BUTTON 0:27, 1:33, 2:4 ·
ENCODER 0:23/19, 1:16/17 · LED_STRIP 0:21. Clutch default OFF (koi
record nahi), shifter sequential (koi SHIFTER record nahi). Purana
7-record factory blob (BUTTON slots 0..3) legacy hai — boot par naye
factory blob se migrate hota hai. Fixed pins (blob mein nahi):
accel/brake button-mode pins 25/26 — GET_CAPS mein hamesha aate hain.

> **Type 11 SHIFTER (Phase F WS5; layout Phase G C3 mein code se verify —
> `pinmap.h`, `config_transaction.h`):** `params[0]` = gate mode
> (`0` Sequential, `1` H-pattern; koi aur value → reject). Gear pins:
> `pinA` = gear 1, `pinB` = gear 2, `pinC` = gear 3, `params[1]` = gear 4,
> `params[2]` = gear 5, `params[3]` = gear 6 (H-pattern mein 6 pins
> chahiye). Validator: mode 0/1, gear pins range + unique, H-pattern +
> ENCODER records → `PIN_CONFLICT`. Sequential mode mein SHIFTER record ki
> zaroorat nahi — BUTTON slots 0/1 (gear up/down) istemal hote hain.
> Clutch koi naya type nahi — `AXIS_ADC` slot 3, koi bhi ADC1 pin
> (32/33/34/35/36/39); gear-down `BUTTON` slot 1 hai — dono ek hi pin par
> hon to validator `PIN_CONFLICT` deta hai.

**Transaction (D28):** stage (SET_CFG chunks) → COMMIT → CONFIRM (30 s) →
permanent. Confirm na ho 30 s mein → **rollback** (purani config wapas).
COMMIT reply mein `confirmDeadlineSec = 30` hota hai. COMMIT ke baad
board nayi config **live apply** karta hai (reboot nahi — D28: reboot ho ya
live apply, timer hamesha 30 s). REVERT: staged ya committed-lekin-unconfirmed
config discard karke purani config wapas. COMMIT par validation fail ho to
reply: `status = PIN_CONFLICT (5)` ya `UNSUPPORTED (6)` + `errCount u8` +
`errCode u8 × errCount` (errCode: `1` pin conflict, `2` value out of range,
`3` unknown module/type, `4` chunk/blob CRC mismatch, `5` total size
mismatch). Validation pin rules WP1/WP2 ke mutabiq hai.

### 3.7 LED_TEST pattern format (request payload, 8 B)

| Bytes | Field | Notes |
|---|---|---|
| 0 | patternId | `0 OFF · 1 SOLID · 2 BLINK · 3 CHASE · 4 RAINBOW` |
| 1 | r | Red 0–255 |
| 2 | g | Green 0–255 |
| 3 | b | Blue 0–255 |
| 4–5 | periodMs | u16, blink/chase period (100–5000); SOLID/RAINBOW par ignore |
| 6 | repeats | u8, pattern repeats; `0` = 5 s tak chalao phir stop |
| 7 | reserved | `0` |

Unknown patternId → `UNSUPPORTED`. LED module configured na ho → reply
`UNSUPPORTED`. **R-L9:** LED test fail hoga jab tak GPIO21 digital ke tor
par use na ho — matlab LED strip GPIO21 par digital-out configured hona
chahiye, warna `PIN_CONFLICT` (5). Test khatam hone par strip purani state
par wapas.

**Sink (Phase H WS2 — RMT):** Strip ko ESP32 RMT peripheral chalata
hai (`firmware/rsh/led_sink_rmt.cpp`, hardware timing — bit-bang nahi;
koi external library nahi, ESP-IDF ka built-in legacy RMT driver).
Bitstream encode (`ws2812_rmt.h`): GRB order, MSB-first, T0 400/800 ns,
T1 800/400 ns, reset ≥ 50 us LOW; 16 LEDs ki frame ~511 us, 60 LEDs
~1778 us (200 Hz loop budget 3500 us se neeche; RMT hardware bhejta
hai, CPU block nahi hota). Driver (`led_driver.h`) sink sirf frame
badalne par bulata hai (SOLID 1 write / 5 s). Strip pin config se
badle (expanders → freed pin) to RMT sink us pin par dobara lagta hai.

**Brightness:** Wire format mein koi brightness field nahi — PC app
r/g/b ko brightness% se scale karke bhejti hai (integer math,
`BoardSession.SendLedTestAsync(brightnessPct)`); firmware ko tabdeeli
nahi chahiye.

**SEC-H-10 (Phase H W7, Low):** Total run time cap **60 s** — repeats
255 × period 5000 ms ~21 min tak blink kar sakta tha; ab pattern
60 s ke baad khud all-off ho jata hai (clamp, reject nahi — repeats
255 wire par valid hai; `led_patterns.h: RSH_LED_TEST_MAX_MS`).

### 3.8 PIN_PROBE (request + stream frames)

Request payload (4 B): `pin u8, durationMs u16 (100–10000), mode u8`
(mode `0` = digital level, `1` = ADC mV). Reply: `status u8,
acceptedMs u16` (board ne kitni duration mani; `0` = reject ke saath).
Probe ke dauraan board port 4211 par probe frames bhejta hai (har 100 ms
ek frame, duration khatam hone tak):

| Bytes | Field | Notes |
|---|---|---|
| 0–3 | magic | `"RSHP"` |
| 4 | version | u8 `0x21` |
| 5–6 | seq | u16 (probe sequence, is probe ke andar 0 se) |
| 7 | pin | u8 (probe ho raha pin) |
| 8 | mode | u8 (0 digital / 1 ADC) |
| 9–10 | sampleCount | u16 (is frame mein samples, 1..64) |
| 11… | samples | mode 0: u8 × N (0/1 level); mode 1: u16 LE × N (mV) |
| last 2 | CRC16 | §0 ke mutabiq |

Probe ke dauraan normal data packets **rukte nahi** — probe frames alag
magic se aate hain. Doosra PIN_PROBE chalu probe ke dauraan → `BUSY` (7).
Pin forbidden/strapping ho (WP2 rules) → `PIN_CONFLICT` (5).

### 3.9 Error table (reply payload ka pehla byte — numeric)

| Code | Name | Matlab |
|---|---|---|
| 0 | OK | Kamiyab |
| 1 | BAD_TAG | HMAC tag verify fail (ghalat key / ghalat nonce / tampered frame) |
| 2 | REPLAY | Counter ≤ session max-seen, naya reqId (replay) |
| 3 | BAD_CRC | SET_CFG chunk ya config blob CRC mismatch |
| 4 | TOO_BIG | `len` > 1024, ya chunk `offset + chunkLen > total`, ya total > 2048 |
| 5 | PIN_CONFLICT | Pin validation fail (conflict / forbidden pin / LED_TEST GPIO21 rule) |
| 6 | UNSUPPORTED | Unknown cmd, unsupported feature, KEY_RESET jab armed na ho, LED module na ho |
| 7 | BUSY | Board busy (flash commit / probe chal raha / confirm window mein conflicting op) |

v2 draft ke string/status codes (`0 ok · 1 bad HMAC · 2 bad frame…`)
**superseded** — sirf ye numeric table authority hai.

## 4. Key / password lifecycle (contract hissa — WP4 docs ke saath parho)

### 4.1 Device key (OPEN-2 — CLOSED)

- **32 bytes**, per-device random, board pehle boot par hardware RNG se
  generate karta hai, NVS mein store hoti hai (plaintext — encrypted NVS
  abhi OPEN-3 hai, Phase H; `docs/KEY-LIFECYCLE.md` §2 dekho).
- Sirf **pehli HELLO** mein (pairing window 60 s ke andar) app ko di jati
  hai (§2.2). Window ke baad kabhi nahi.
- App DPAPI (CurrentUser) se store karti hai; kabhi plain text / log / UI /
  export mein nahi aati. Koi backup/export/cloud sync nahi.
- Key rotation sirf KEY_RESET, re-pairing, board replace, ya poora reflash
  par hoti hai. Reboot / OTA / FACTORY_RESET par **nahi**. Key ki koi expiry
  nahi.

### 4.2 Pairing bootstrap (OPEN-6 — CLOSED, **BLOCKER — coordinator review**)

**Faisla: pehli pairing ke liye SoftAP khula (open, koi password nahi) hota
hai jab tak board pair na ho jaye. Pairing kamyab hote hi (pehli HMAC-signed
GET_CAPS OK) board AP ko WPA2-PSK par band kar deta hai, password wahi
per-device password hota hai jo HELLO mein diya gaya tha.**

Reasoning:
- Board ke paas **koi display, button ya label printer nahi** hai — default
  password dikhane ka koi rasta nahi hai (label par factory password chhapna
  "koi factory nahi" usool ke khilaf hai aur shared-default ban jata).
- USB-serial se pehli pairing primary rasta nahi ho sakta: us ke liye cable
  + driver + alag spec (OPEN-20, Phase G) chahiye, aur har user ke paas
  cable nahi hoti. USB pairing Phase G mein **optional** rasta ho sakta hai.
- Khula AP sirf **pehle boot se pairing tak** rehta hai (60 s window +
  pairing complete hone tak; pairing complete na ho to window khatam hone
  par board AP band kar ke dobara kholta hai? Nahi — dekho neeche).

**Security asar (honest):** pairing window mein link-layer encryption
**nahi** hoti — key + password ka tabadla proximity (Wi-Fi range) + 60 s
window par munhasir hai. Window ke andar range mein maujood hamlaawar
tabadla sun sakta hai (THREAT-MODEL TM-W02 residual risk). Mitigations:
(a) window sirf 60 s, AP-up se; (b) disclosure ek dafa; (c) pairing sirf
apne ghar/room mein karne ki hidayat USER-GUIDE mein lazmi (Phase H);
(d) pairing ke foran baad AP WPA2 ho jata hai aur key dobara kabhi nahi di
jati; (e) agar user ko shak ho to KEY_RESET (§4.4) se nayi key+password.
**Window expire hone par (pairing na ho):** board AP **khula hi rehta hai**
lekin HELLO mein key/password **zero** bhejta hai (flags bit3 = 0) aur
pairing refuse karta hai — dobara pairing ke liye KEY_RESET lazmi hai.
(Khula AP rehne ka matlab sirf ye hai ke board nazar aata hai; bina key ke
us se kuch control nahi ho sakta.)

### 4.3 FACTORY_RESET vs KEY_RESET

|  | FACTORY_RESET (`0x08`, HMAC-signed) | KEY_RESET (`0x0B`, physical) |
|---|---|---|
| Wipe | Sirf pin map / config (factory defaults = v3.3 pin map) | Sirf device key + pairing state (+ password, §5.3) |
| Key | **Rehti hai** — purani key se HMAC valid rehta hai | **Wipe + nayi key generate** — purani foran invalid |
| Password | **Rehta hai** (§5.3) | **Naya generate** hota hai (§5.3) |
| Auth | HMAC-signed hona lazmi (purani key se) | Physical BOOT-hold lazmi; purani key ki zaroorat **nahi** |

### 4.4 KEY_RESET + BOOT-hold (OPEN-9, OPEN-10, OPEN-8 — CLOSED)

- **Cmd byte: `0x0B`** (OPEN-10 — CLOSED). FACTORY_RESET `0x08` barqarar.
- **BOOT-hold ka farq (OPEN-9 — CLOSED):** dono istemal **hold ki muddat**
  se farq karte hain, power-up par GPIO0 (BOOT) hold karke:
  - **5 s ≤ hold < 10 s → safe mode** (factory pin map + AP; purana
    behaviour). LED: slow blink, 1 Hz.
  - **hold ≥ 10 s → key-reset-armed state.** LED: fast blink, 4 Hz.
  - Hold < 5 s → normal boot (BOOT ignore).
  - Board hold duration power-up ke waqt naapta hai (BOOT release ka
    waqt); band edges par ±0.5 s tolerance hai — firmware Phase D mein
    real board par measure karega (A7 gate).
- **Armed state (OPEN-8 — CLOSED):** armed state ki pehchan LED fast blink
  (4 Hz) + HELLO/GET_CAPS flags bit `keyResetArmed = 1` hai. Armed window
  **120 s** hai (boot se); waqt guzar jaye to board normal mode mein reboot
  hota hai — dobara BOOT-hold karna hoga. Armed state mein (aur sirf is
  mein) app `KEY_RESET` bhejti hai; armed na ho to reply `UNSUPPORTED` (6).
- KEY_RESET kamyab hone par: board purani key mitata hai, **nayi key +
  naya password** generate karta hai, unpaired mark hota hai, **nayi 60 s
  pairing window** kholta hai (§2.2/§4.2 se pairing dobara). Config **nahi**
  badalti.
- **Safe mode mein KEY_RESET nahi hota** — safe mode sirf factory pin map +
  AP deta hai; key wahi rehti hai.

### 4.5 Key kho jaye / re-pairing

App reinstall / DPAPI key kho jaye / naya PC / board kisi aur ko dena →
§4.4 KEY_RESET → nayi key → §2.2/§4.2 pairing dobara. **Hamesha nayi key**
banti hai; purani dobara kabhi valid nahi hoti; koi "purani key recover"
ka rasta nahi hai. Board replace hone par naye board se pehli pairing
(§2.2); profile v2 mein `boardMac` (D21) se app pehchan sakti hai ke board
badla hai — calibration dobara karni hogi (warning ke saath).

### 4.6 RNG failure / NVS (OPEN-4 — CLOSED behaviour, mechanism Phase D)

**Fail closed:** agar pehle boot par RNG tayyar na ho ya key generation
fail ho, board **normal boot mein aage nahi barhta**. Tareeqa: RNG init
3× retry; phir bhi fail ho to board fault state mein rehta hai — LED
fault pattern (3 short blinks, repeat), AP up nahi hota, koi key nahi
banti, koi pairing nahi. Kamzor ya repeat key ke saath boot **kabhi** aage
nahi barhega. ESP-IDF RNG API + NVS encryption ka exact mechanism
(OPEN-3) Phase D ka hai — contract sirf ye behaviour mangta hai: key /
password **kabhi plain flash mein nahi**, hamesha encrypted NVS mein.
> **Correction (Phase F W1 — SEC-E-01):** aakhri jumla abhi sach nahi hai.
> Asal code plaintext NVS istemal karta hai (OPEN-3 khula hai, Phase H);
> password ke liye Phase F obfuscation (`"wifipw2"`) hai — encryption
> nahi. Contract ka asal mutalba (flash dump se key/password na mile)
> **Phase H mein flash encryption se** poora hoga; tab tak residual risk
> TM-W07 hai.

## 5. Wi-Fi / OTA password (OPEN-13/14/15/16 — CLOSED)

### 5.1 Ek credential, format (OPEN-13, OPEN-14 — CLOSED)

- **Faisla (OPEN-13): Wi-Fi (AP join) aur OTA (WebOTA auth) ke liye EK hi
  credential hai** — ek per-device password. Reasoning: dono auth ek hi
  board, ek hi local trust boundary (AP) ke hain; do alag passwords ka
  matlab double provisioning, double dikhana, double bhoolna — user burden
  bina security faide ke. Compromise ka asar bhi ek jaisa hai (dono local
  hain). Is liye ek password, do jagah istemal.
- **Faisla (OPEN-14): 24 characters**, charset `A–Z, a–z, 2–9` — ambiguous
  characters `0, 1, I, O, l` **nikal diye gaye** (57 symbols — gin lo:
  24+25+8; pehle "58" likha tha, wo ghalat tha, SEC-D follow-up mein
  theek hua). Entropy ≈ `24 × log2(57) ≈ 140 bit`. Generation: hardware RNG, uniform (modulo bias
  se bachne ke liye rejection sampling — Phase D). WPA2-PSK minimum 8
  characters se zyada hai; 24 user ke liye type karne ke qaabil hai.
- **Display:** app password ko pehli pairing par **ek dafa** QR / label ke
  tor par dikhati hai — 4 groups of 6, hyphen se (`XXXXXX-XXXXXX-XXXXXX-
  XXXXXX`). **Wire/store format mein hyphen nahi hote** — HELLO reply
  (§2.2) aur NVS mein 24 ASCII bytes, baghair hyphen.
- **Forced change (NIST 8259A):** pehli pairing ke baad user ko apna
  password set karne ka mauqa/taqaza hoga — exact UX Phase D/E (OPEN-12),
  mechanism `SET_PASSWORD` (§5.2) yahan define hai.

### 5.2 SET_PASSWORD (`0x0C`) — mechanism

- Request: `newLen u8 (8..63)` + password ASCII bytes. Charset: printable
  ASCII `0x21–0x7E` (space nahi). Length/charset fail → `UNSUPPORTED` (6).
- Kamyabi par board password AP + WebOTA dono ke liye apply karta hai aur
  AP restart karta hai (connected clients drop honge — dobara join karna
  hoga). Reply `OK` restart se **pehle** bheja jata hai.
- SET_PASSWORD HMAC-signed hona lazmi hai (purani key se) — bina key ke
  password nahi badal sakta.

### 5.3 App-side storage (OPEN-15 — CLOSED, Phase F correction)

> **Correction (Phase F W1, 2026-11-13 — SEC-E-04):** is section par
> "CLOSED" Phase E se likha tha, lekin implementation ult thi: DPAPI blob
> (`KeyStore.TrySavePassword`) likha jata tha, parha kabhi nahi jata tha —
> app asal mein plaintext `WheelWifiPassword` (`settings.json`) istemal
> karti thi, aur `ChangePassword` + backup zip do mazeed plaintext raste
> thay. Doc-vs-code contradiction thi; Phase F mein **code ko doc ke
> mutabiq kiya gaya** (doc ka faisla sahi tha, implementation ghalat thi).
> Ab sach ye hai:
>
> - **Read path:** sirf DPAPI KeyStore (`BoardSession.TryGetWifiPassword`,
>   board MAC se) — `settings.json` se password kabhi nahi parha jata.
> - **Write path:** `SET_PASSWORD` kamyaabi par KeyStore mein save hota
>   hai (`BoardSession.SetPasswordAsync`); Wi-Fi join kamyaabi par bhi
>   (`ConnectionViewModel.ConnectWifi`); `settings.json` mein kabhi nahi.
> - **Migration:** purani `settings.json` mein plaintext mile to pehle
>   HELLO par KeyStore mein migrate hota hai
>   (`AppServices.TryMigrateWifiPassword`), phir settings dobara save ho
>   kar disk se saaf ho jata hai. Board kabhi connect na ho to legacy
>   value sirf us session ki memory mein rehti hai; agla Save disk wali
>   copy bhi mita deta hai (password dobara darj karna hoga).
> - **Backup:** `rsh-backup-*.zip` mein `settings.json` live settings se
>   serialize hoti hai (`AppSettingsStore.ToBackupJson`) — password fields
>   `[JsonIgnore]` hain, is liye backup mein password kabhi nahi aata.
> - **Rotation (SEC-E-09):** nayi device key (KEY_RESET / nayi pairing)
>   purana stored password stale kar deti hai — `UpdateFromHello` use
>   foran mitata hai. `SET_PASSWORD` ke baad store save fail ho to
>   `(true, warning)` milta hai (board badal gaya hai — jhoot nahi), aur
>   warning mein KEY_RESET recovery likhna lazmi hai.
> - **Board side (SEC-E-01):** password NVS mein ab obfuscated hai
>   (`"wifipw2"`, device key se keystream XOR — `pw_obfuscate.h`); ye
>   **obfuscation hai, encryption nahi** (device key khud usi NVS mein
>   plaintext hai). Asal hal flash encryption / encrypted NVS hai
>   (OPEN-3) — Phase H, `KEY-LIFECYCLE.md` §2 dekho.

- **Wi-Fi join:** OS Wi-Fi profile (Windows WLAN profile) — app password
  ko apni file mein **nahi** rakhti join ke liye; OS profile is ka ghar hai.
- **WebOTA auth ke liye:** app password ko **DPAPI-encrypted blob** mein
  store karti hai (device key wali store ke saath, `%AppData%\RSH` mein,
  alag blob). Reasoning: Windows WLAN profile se PSK plaintext wapas
  nikalna public API se nahi hota (admin `netsh` chahiye) — WebOTA auth ke
  liye app ko password chahiye, is liye DPAPI blob zaroori hai. Protection
  level device key jaisa hi hai (user-bound; §4.1).
- Password kabhi plain file / log / UI (ek dafa dikhane ke baad) / export /
  crash dump mein nahi aata. In-memory sirf zaroorat ke waqt, app band
  hone par clear.

### 5.4 Password bhool jaye to (OPEN-16 — CLOSED)

- **Faisla: KEY_RESET (§4.4) password ko bhi reset karta hai.** KEY_RESET
  par board **nayi key + naya password dono** generate karta hai, phir
  dobara pairing (§4.2). FACTORY_RESET password ko **nahi** chheerta
  (credentials config nahi hain — key wale rule jaisa).
- Reasoning: password reset ko physical presence chahiye (warna jis ke
  paas key ho wo password badal kar malik ko lockout kar sakta hai —
  FACTORY_RESET bhi HMAC-signed hai lekin wo config ka kaam hai,
  credentials ka nahi). KEY_RESET hi woh rasta hai jo key kho jane aur
  password bhoolne dono ko ek physical action se hal karta hai.

## 6. Purani doc se farq (OPEN-1 aur v2 draft supersede list)

### 6.1 OPEN-1 — CLOSED: key derivation badal gayi

Purani `PROTOCOL-V2.md` mein `key = SHA-256(OTA password)` tha aur tag 8
bytes ka tha. **Ye superseded hai.** v2.1 mein key **per-device random
32 bytes** hai (board generate karta hai, §4.1), tag **16 bytes** hai, aur
HMAC input `nonce ‖ counter ‖ cmd ‖ payload` hai (password is mein kahin
nahi aata). `PROTOCOL-V2.md` ko is design ke mutabiq update kar diya gaya
hai — v2.1 ke liye **ye doc authority** hai.

### 6.2 v2 draft se badla hua (summary)

| Cheez | v2 draft (`PROTOCOL-V2.md` purani) | v2.1 (ye doc) |
|---|---|---|
| Stream magic | `0xA5` + ver byte | `"RSH2"` + ver `0x21` |
| Stream flags | bit0 steering-mV | D4 table (§1.3) |
| Buttons | u32 (draft mein bhi) | u32 + `btnsEx` status mein (D1) |
| Encoders | i8 detents | i16 cumulative |
| Axes | u16 (mvMask tha) | u16 mV, mvMask khatam (D6) |
| Status | 30 B | 34 B base / 38 B extended (D5) |
| Control magic | `0xC7` + ver 1 | `"RSHC"` + cmd + reqId + counter + len |
| Control tag | HMAC[0:8], key=SHA256(pw) | HMAC[0:16], key=random 32 B |
| Control counter | nahi tha | u32 per-session (D3) |
| Error codes | string-ish 0–6 | numeric 0–7 (§3.9) |
| HELLO reply | nahi tha | 92 B fixed (§2.2) |
| GET_CAPS body | "board id, limits, pin table" (no layout) | byte layout (§3.4) |
| SET_CFG | chunk(offset,total,bytes) (no layout) | byte layout + CRC16 (§3.5) |
| LED_TEST | "pattern" (no layout) | 8 B pattern (§3.7) |
| PIN_PROBE | "pin, ms" (no stream frame) | request + `"RSHP"` stream frame (§3.8) |
| Config blob magic | `"RSHC"` | `"RSHG"` (§3.6) |
| KEY_RESET / SET_PASSWORD | nahi thay | `0x0B` / `0x0C` (§3.3) |
| Confirm timer | 10 s (draft) | **30 s** (D28) |

## 7. OPEN items ka status (WP3)

| # | Item | Status | Faisla / wajah |
|---|---|---|---|
| OPEN-1 | Key = SHA256(OTA pw) vs per-device random | **CLOSED** | §6.1 — per-device random 32 B; `PROTOCOL-V2.md` update ho gayi |
| OPEN-2 | Key length | **CLOSED** | §4.1 — **32 bytes** (HMAC-SHA256) |
| OPEN-3 | Encrypted NVS mechanism + flash encryption on/off | **OPEN — Phase H** (procedure + secure-build files Phase H W1 mein ban gaye: `docs/FLASH-ENCRYPTION-PROCEDURE.md`, `partitions-secure.csv`, `sdkconfig.secure.defaults`, `[env:esp32dev-secure]`, `secure_nvs.h` runtime check; board execution + closure criteria procedure §5/§8 mein) | Contract sirf behaviour mangta hai (§4.6: kabhi plain flash nahi — **abhi poora nahi hua**, residual TM-W07). ESP-IDF mechanism firmware ka faisla hai |
| OPEN-4 | RNG tayyari + key-gen failure behaviour | **CLOSED** (behaviour) | §4.6 — fail closed, 3× retry, phir fault state (LED 3-short-blink). API/mechanism Phase D |
| OPEN-5 | Pairing window start trigger | **CLOSED** | §2.3 — AP-up se 60 s; expiry nishani flags bit3 + zero fields |
| OPEN-6 | Pairing bootstrap circular dependency | **CLOSED** (**BLOCKER**, coordinator review) | §4.2 — pehli pairing tak AP khula; phir WPA2. Security asar likha hai |
| OPEN-7 | HELLO reply byte layout | **CLOSED** | §2.2 — 92 B fixed, nonce 16 B samet |
| OPEN-8 | KEY_RESET armed state pehchan + window | **CLOSED** | §4.4 — LED fast blink 4 Hz + flags bit; window 120 s |
| OPEN-9 | BOOT-hold do istemal | **CLOSED** (coordinator review) | §4.4 — hold duration: 5–<10 s safe mode, ≥10 s key-reset-armed |
| OPEN-10 | KEY_RESET cmd byte | **CLOSED** | §3.3/§4.4 — **`0x0B`** |
| OPEN-11 | OTA key NVS ko nahi chheerti | **OPEN — Phase D** | Contract requirement: OTA NVS key namespace ko preserve kare. Firmware mein confirm karna hai |
| OPEN-12 | Forced password change UX/flow | **OPEN — Phase D/E** | Mechanism `SET_PASSWORD` (§5.2) define hai; app UX + board apply flow Phase D/E |
| OPEN-13 | Wi-Fi vs OTA password: ek ya do | **CLOSED** | §5.1 — **ek credential** dono ke liye |
| OPEN-14 | Password length / charset | **CLOSED** | §5.1 — **24 chars**, `A–Z a–z 2–9` (57 symbols, ≈140 bit) |
| OPEN-15 | App-side password storage | **CLOSED** (Phase F correction — Phase E ka "CLOSED" daawa ghalat tha: DPAPI blob likha jata tha, parha nahi jata tha; app plaintext `settings.json` istemal karti thi. Phase F W1 mein code doc ke mutabiq hua: read/write sirf KeyStore, migration + backup fix — §5.3 dekho) | §5.3 — Wi-Fi: OS profile; WebOTA: DPAPI blob (key store ke saath) |
| OPEN-16 | Password bhoolne par reset rasta | **CLOSED** | §5.4 — **KEY_RESET** (key + password dono naye); FACTORY_RESET nahi |
| OPEN-17 | App-level signature: Ed25519 vs ECDSA P-256 | **CLOSED** (coordinator review) | **Ed25519** — 64 B signatures, deterministic verify, verify ke liye RNG nahi chahiye, ESP-IDF/mbedTLS support. ECDSA P-256 alternate nahi — ek algorithm, wahi verify-before-apply |
| OPEN-18 | Anti-downgrade exact mechanism | **CLOSED** (rule) / mechanism **Phase D** | Rule: OTA image ka version (fwMajor/Minor/Patch, image header mein) running version se **kam ho to reject**. eFuse counter istemal karna hai ya nahi — Phase D |
| OPEN-19 | Signing/Secure Boot key compromise response | **OPEN — Phase H** | Release process ka kaam (revocation / re-sign / purane boards) |
| OPEN-20 | Wired (USB-serial) par pairing | **CLOSED** (Phase G WS1) | §10.5 — USB cable trust anchor hai; HELLO key disclosure + HMAC GET_CAPS confirm serial par (Wi-Fi pairing §4.2 unchanged) |

## 8. Verification status

- **Golden vectors:** `tests-host/protocol_v2_vectors.py` — stream v2.1,
  status 34 B + 38 B, HELLO reply 92 B, control HMAC (positive + negative:
  bad tag, wrong key, replay, truncated, duplicate reqId) — sab regenerate
  hue hain. Script chalana = vectors produce karna (implementation), **gate
  pass nahi**.
- **3-language match gate (T1/T2/T3): PENDING — Virtual Test Rig ka
  intezar.** Rig (`~/workspace/projects/rsh/test-env/`) abhi tayyar nahi
  hai. Is doc ke bahar alag test setup **nahi** banaya gaya. Gate sirf rig
  ke andar dobara green hone par "pass" likha jayega.
- **Coordinator review ke liye flagged:** OPEN-6 (bootstrap), OPEN-9
  (BOOT-hold bands), §1.2 status offsets note, §1.1 N1 (buttons 64–127),
  OPEN-17 (Ed25519).

## 9. NTP-style ping (Phase C, SW-13) — E2E latency measurement

Status: **CONTRACT — Phase C deliverable.** Plan v2.3 §3 (R-H2): E2E latency
**sirf** isi ping se measure hoti hai; jab tak ping na ho, SLO "unmeasured".
Security requirements: `docs/SECURITY-REVIEW-PHASE-C.md` §4
(SEC-C-21/22/23/24/25) — neeche §9.4–§9.6 mein wahi numbers/rules contract
mein likhe gaye hain. **Board-side reply firmware Phase D mein aayega;**
tab tak virtual wheel (`test-env/virtual-wheel/wheel.py`) ping reply deta
hai (wahi frame, wahi rate limit) taake PC side rig mein test ho sake.

### 9.1 Frame layout — fixed 34 B (request = reply, dono)

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHN"` | ASCII `0x52 0x53 0x48 0x4E` — stream (`"RSH2"`), control (`"RSHC"`), probe (`"RSHP"`) se alag |
| 4 | version | u8 | `0x21` (= v2.1, §0) |
| 5 | type | u8 | `0` = request (PC → board), `1` = reply (board → PC). Aur koi value → drop |
| 6–7 | seq | u16 | PC chunta hai, har ping +1 (wrap allowed); reply wahi seq echo karta hai |
| 8–15 | t0 | u64 | PC send, PC monotonic µs (Stopwatch). Request aur reply dono mein wahi value (echo) |
| 16–23 | t1 | u64 | Board receive, board monotonic µs (boot se). Request mein `0` |
| 24–31 | t2 | u64 | Board send, board monotonic µs (boot se). Request mein `0` |
| 32–33 | CRC16 | u16 | §0 ke mutabiq (bytes 0–31 par) |

Total **34 B fixed** — request aur reply dono. Length `!= 34` → drop + count
(header parha ja sake to `BadLength`, 8 B se chhota ho to `TooShort` —
control §3.1 jaisa rule). Timestamps **u64 µs** hain: u32 µs 71 min mein wrap
hota — ping mein wrap nahi chahiye, is liye firmware board clock ko u64
monotonic rakhega (boot se µs). PC aur board ke epochs alag hain — offset
formula hi un ka farq nikaalta hai; delay par epoch ka koi asar nahi.

### 9.2 NTP formulas (PC side)

t3 = PC receive ka waqt (wire par nahi aata — PC reply milte hi khud naapta hai).

- **delay** = `(t3 − t0) − (t2 − t1)` — round-trip minus board processing.
  Yehi E2E network delay hai; SLO isi par lagta hai (p99 ≤ 10 ms Wi-Fi /
  ≤ 5 ms wired, v2.3 §3).
- **offset** = `((t1 − t0) + (t2 − t3)) / 2` — board clock − PC clock (µs).
- **board processing** = `t2 − t1` (diagnostics ke liye).

PC reply tab hi maanta hai jab: magic/version/type/length/CRC sahi hon,
`type = 1` ho, `seq` aur `t0` bheje gaye request se match karein, aur
`t2 ≥ t1`, `t3 ≥ t0`, `delay ≥ 0` hon. Warna reply **invalid** hai — metric
mein nahi daala jata, sirf gin'a jata hai. Timeout (500 ms, koi retry nahi —
agla ping 1 s baad aata hai) par metric **"unmeasured"** rehta hai, crash nahi.

### 9.3 Board-side reply rules (firmware, Phase D)

- Request milte hi foran reply: t1 = receive karte waqt ka board clock,
  t2 = send karte waqt ka board clock. Reply banane mein **koi heap
  allocation nahi, koi state store nahi** — sirf frame bharo aur bhejo.
- Reply **sirf packet ke asal source address/port** par bhejo (UDP
  `recvfrom` se mila hua). Payload ke andar koi address hota hi nahi —
  frame mein address ka field hai hi nahi, is liye payload se address
  lene ka rasta maujood hi nahi (SEC-C-22).
- **Firmware status (Phase D, SEC-D-09):** board-side ping handler +
  per-source token bucket implement hain (`net.cpp` `handle_ping`,
  `net_logic.h` `rsh_ping_limiter_t` — §9.4 ke numbers). Flood negative
  test host tests mein hai; rig ka `ntp-ping` scenario PC side + virtual
  wheel ka reference limiter check karta hai.
- **Ping se koi state change nahi:** ping HELO ka 2 s silence timer reset
  **nahi** karta, koi config/session/counter nahi badalta, koi stream
  packet nahi rokta. Ping sirf jawab deta hai (SEC-C-25). Busy ho (stream
  loop peeche ho) to ping **drop** karo — stream loop ko kabhi na roko.
- Reply mein sirf seq + 4 timestamps hote hain — **koi key, nonce,
  password, ya high-precision uptime nahi** (SEC-C-23). Koi variable-size
  token nahi hai (seq u16 hi echo token hai, fixed size).

### 9.4 Board-side rate limit — SEC-C-21 (High)

Ping unauthenticated UDP hai (hona hi chahiye — ye measurement hai, control
nahi; pairing se pehle bhi chalna chahiye). Bina limit ke board reflector /
DoS vector ban jata hai (TM-W05), is liye:

- **Per-source token bucket: max 10 replies/s, burst 5** (bucket capacity 5,
  refill 10 tokens/s). Key = **source IP address** (port nahi).
- Limit se zyada ping **khamoshi se drop** karo — koi error reply nahi
  (error reply bhi amplification hai). Drop ko gino (diagnostics), lekin
  drop ki wajah se koi packet mat bhejo.
- Bucket table **bounded** hai (fixed-size table, misal 64 sources) —
  attacker sources ki list ko flood na kar sake. Table bhar jaye to sab se
  purana source nikaal kar naya rakho.
- Reference implementation: `RSH.Core/Transport/PingRateLimiter.cs`
  (wahi numbers, unit tests ke saath); virtual wheel mein wahi rule Python
  mein hai. Firmware Phase D mein is se match karega.

### 9.5 PC-side rules — SEC-C-24 (Low) + SEC-C-22 (Medium)

- **Sender 1–4 Hz:** normal loop 1 Hz; `NtpPingClient` do pings ke darmiyan
  kam se kam **250 ms** ka waqfa lazmi rakhta hai (max 4 Hz) — is se tez
  bulaya jaye to packet bheje baghair null milta hai (throttled, timeout nahi).
- **Ephemeral port (T10):** ping socket bind nahi hota — OS ephemeral port
  deta hai. Timeout 500 ms, koi retry nahi; jawab na aaye to "unmeasured"
  dikhao, board-failure nahi.
- **Reply ≤ request (SEC-C-22):** dono fixed 34 B hain — reply request se
  bara kabhi nahi, is liye amplification factor ≤ 1 hai. Per-source rate
  limit (§9.4) reflection ka asal rok hai.
- **HELO-gating ka faisla (SEC-C-22 sakht option — NAHI liya gaya):**
  jawab sirf un sources ko dena jinhon ne haal mein valid HELO bheja ho —
  ye option **reject** kiya gaya hai, kyun ke ping ko pairing/HELO se pehle
  bhi measurement ke liye chalna hai. Is ke bajaye rate limit (§9.4) +
  fixed size hi rok hain. Ye faisla yahan darj hai taake khamoshi se na badle.
- Ping kabhi UI thread par nahi chalta (async); raw ping payload kabhi log
  nahi hota.

### 9.6 Clock offset — SEC-C-25 (Low)

- Offset **sirf SLO display** (E2E latency panel) ke liye hai. Ping
  timestamps **untrusted measurement** hain — in se koi auth / replay /
  expiry / koi security faisla **nahi** hoga. Code mein `PingResult.OffsetUs`
  ka waahid consumer `E2ELatency` (display) hai — isay koi security path
  istemal nahi karta, aur aisa karna is contract ki khilaf-warzi hogi.
- Board ka waqt monotonic hai (wall clock nahi) — NTP adjustment se nahi hilta.

### 9.7 Negative tests (rig mein — `ntp-ping` scenario + unit tests)

- Flood: 100 ping/s ek source se → pehle 5 replies, baqi silent drop;
  board/stream 200 Hz barqarar (`PingRateLimiter` unit tests + wheel self-check).
- Spoofed source: reply sirf asal source par, size ≤ request (34 B = 34 B).
- Ghalat length / ghalat magic / bara token: frame mein variable token hai
  hi nahi; ghalat length/magic → drop + count, crash nahi.
- Timeout: board jawab na de → metric "unmeasured", crash nahi; board
  wapas aaye to measurement dobara shuru.
- PC flood: `PingAsync` ko 200 Hz par bulana → throttle (250 ms), board
  tak flood nahi pahunchta.

## 10. Wired USB-serial transport (D26 / OPEN-20, Phase G)

Status: **CONTRACT — Phase G WS1 deliverable** ("Wired spec + working link",
plan v2.3 §5 D26 — spec ab TBD nahi). Wi-Fi/UDP ke saath **dual transport**:
wahi v2.1 payloads, wahi session/auth rules — sirf envelope aur medium alag
hai. Koi naya wire format nahi — serial sirf UDP payloads ko lapet ta hai.

### 10.1 Frame format — "RSHS" envelope

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHS"` | ASCII `0x52 0x53 0x48 0x53` |
| 4 | kind | u8 | §10.1 table (1..7); aur koi value → BadLength |
| 5–6 | len | u16 LE | Payload length (0..1053) |
| 7.. | payload | bytes | Wahi bytes jo UDP par jate (§1/§2/§3/§9) |
| last 2 | CRC16 | u16 LE | CRC16-CCITT (§0: poly 0x1021, init 0xFFFF) — CRC field se pehle ke tamam bytes par (D2) |

Max payload **1053 B** (bara control frame, §3.1) → max frame **1062 B**.
Header 7 B, CRC 2 B.

Kinds (payload wahi jo UDP par — koi naya format nahi):

| Kind | Name | Direction | Payload |
|---|---|---|---|
| 1 | STREAM | Board → PC | v2.1 `"RSH2"` stream packet (§1.1) ya v1 18 B packet |
| 2 | STATUS | Board → PC | v2.1 status packet (§1.2, 34/38 B) |
| 3 | HELLO | Board → PC | HELLO reply (§2.2, 92 B) |
| 4 | CONTROL | Dono taraf | `"RSHC"` control frame (§3.1, max 1053 B) |
| 5 | PING | Dono taraf | `"RSHN"` ping frame (§9, 34 B) |
| 6 | HELO | PC → board | HELO request (§2.1, 5 B `"HELO"` + proto + sessionFlag) |
| 7 | PROBE | Dono taraf | `"RSHP"` probe frame (§3.8) |

Canonical vectors (§10.1 — 3-language match: C# `SerialFrameCodec`,
C++ `serial_transport.h` host tests §22, Python `protocol_v2_vectors.py`
— teeno byte-exact):

- Stream, payload `01..08` → `525348530108000102030405060708d95d` (CRC 0x5DD9)
- Helo, payload `"HELO" 02 00` → `5253485306060048454c4f02004749` (CRC 0x4947)

Parse error rules (control §3.1 jaisa — B1 rule): **TooShort sirf tab jab
7 B header bhi poora na ho**; header parh liya gaya ho to har length
mismatch (truncation, len > 1053, kind unknown, extra byte) → **BadLength**;
CRC mismatch → **BadCrc**; magic ghalat → **BadMagic**.

### 10.2 Decoder rules (incremental, dono taraf ek jaisi)

- Byte-at-a-time: Serial se jese bytes aain, wese feed karo. Partial frame,
  chunked delivery aur beech mein garbage (boot log / bootloader output)
  sab sambhalta hai — ghalat magic/CRC/length par resync, **frame kabhi
  aadha deliver nahi hota**.
- Ghalat frame **khamoshi se gira** diya jata hai aur gin'a jata hai
  (`BadMagic` / `BadCrc` / `BadLength` counters) — koi error reply nahi.
- **Dropped-frame attribution:** frame gira to bhi, agar header parh liya
  gaya tha (kind maloom hai) to gira hua frame apne kind ke saath gin'a
  jata hai — C# `SerialFrameDecoder.FrameDropped` (kind + reason
  BadCrc/BadLength). Is ka waahid consumer abhi `SerialLink.HelloBadCount`
  hai: **bad-CRC HELLO frame → `BadCrc` bhi 1, `HelloBadCount` bhi 1**
  (frame deliver nahi hota, lekin HELLO kharab hai — ye gin'na lazmi hai).
  Magic scan mein gire bytes ka kind maloom nahi hota — wo sirf `BadMagic`
  mein rehte hain. Firmware side (`serial_link.cpp`) mein HELLO kabhi
  inbound nahi aata (board HELLO *bhejta* hai), is liye wahan ye rule
  laagu nahi — sirf `bad_frames` counter hai.
- HELLO payload parse fail (valid frame CRC, lekin payload magic/version/
  length/CRC ghalat) → bhi `HelloBadCount` (dispatch ke baad).

### 10.3 Physical layer — CP2102 USB-UART bridge

- ESP32 DevKit V1 par **native USB nahi hai** — USB-serial ka matlab
  CP2102 USB-UART bridge (`Serial`) hai. S3 ka native USB OTG is spec ka
  hissa nahi (Phase G/H HIDMaestro evaluation ke saath future path).
- **921600 baud, 8N1**, no handshake, no flow control. 115200 kaafi nahi:
  200 Hz × ~45 B frame ≈ 9000 B/s = 115200 baud ka ~78% — control
  replies/status ke liye headroom nahi. 921600 par ~10% utilization.
- **DTR/RTS KABHI assert nahi hote** (`DtrEnable`/`RtsEnable` = false):
  DevKit V1 par DTR/RTS EN/BOOT lines se jure hote hain — assert karne par
  board reset ho jata hai ya bootloader mein chala jata hai. Port kholna
  board ko reset **nahi** karta (flashing tool jaisa behaviour nahi —
  transport sirf data bhejta hai).
- Cable unplugged ho to CP2102 bytes yunhi leta hai (koi flow control
  nahi) — `Serial.write` block nahi hota; read loop port error par band
  ho kar dobara `Start()` se khulta hai (reconnect).

### 10.4 Dual transport — do peer, do session, ek sequence

- Wi-Fi aur serial **alag peer, alag session** hain: apna HELO timer, apna
  auth state, apna counter (firmware `net_serial_*` — Wi-Fi `net_*` se
  alag). Packet ek dafa banta hai, dono transports ko jata hai (har
  transport ko sirf us ka version: v2 peer ko v2 packet, legacy v1 peer ko
  v1 packet). Dono peer hon to **seq ek hi rehta hai** (ek sequence space).
- Stream emission (200 Hz) **per transport gated** hai (SEC-E-14, har
  transport par alag): serial peer active + v2 + authenticated session
  (pehle ek HMAC-verified control command, aam tor par GET_CAPS) — warna
  serial par kuch nahi jata. Status packet (1 Hz) waisi hi gating se
  serial peer ko jata hai. HELO (unauthenticated bootstrap) se stream
  kabhi nahi khulta — Wi-Fi jaisa hi rule.
- Ek serial link par kayi logical channels: stream/status/probe → events
  (`SerialWheelSource`); control/ping/HELO replies → waiter
  (`SerialControlTransport` / `SerialPingTransport` / `SendHelloAsync`).
  **Ek waqt mein ek hi control exchange / HELO / ping** — doosra caller
  `InvalidOperationException` pata hai (ControlClient sequential hai).
- PC se board ko na chahiye wale inbound frames (STREAM/STATUS/HELLO/
  PROBE PC → board) drop + count hote hain (`DroppedInboundFrames` /
  firmware `serial_link_dropped_inbound()`).
- Board-side poll har call mein zyada se zyada **512 bytes** drain karta
  hai (flood se 200 Hz loop starve na ho — `net_poll` ke 8-datagram cap
  jaisa). Reply banane mein koi heap allocation nahi.
- Reconnect: cable nikalo + dobara lagao → PC dobara `Start()` karta hai
  (port reopen, decoder reset, stale bytes discard); board ka serial
  session HELO timeout ke baad khatam hota hai, naya HELO naya session
  kholta hai (resume flag ke saath — §2.3).

### 10.5 Wired pairing (OPEN-20 — CLOSED)

- **USB cable trust anchor hai:** pairing ke waqt board aur PC ke darmiyan
  physical cable hai — jo cable pakar sakta hai, wo board pakar sakta hai.
  Is liye wired pairing mein **OPEN-6 bootstrap circularity nahi hai**:
  pehli pairing ke liye khula SoftAP nahi chahiye, AP password ka masla
  hi nahi — key seedha cable par milti hai.
- Flow Wi-Fi pairing (§4.2) jaisa hai, transport serial hai:
  1. Unpaired board (pehla boot ya KEY_RESET ke baad) 60 s pairing window
     kholta hai (§2.3 — wahi flags bit3 + window rule).
  2. PC HELO bhejta hai (serial, §10.1 kind 6); board HELLO reply deta
     hai (serial, kind 3) — **92 B, §2.2 layout, key + Wi-Fi/OTA password
     sirf unpaired + window-open hon to** (wahi disclosure rule — §4.1).
  3. App pairing ki tasdeeq karti hai: HMAC-signed GET_CAPS bhejti hai
     (serial control transport par — wahi `BoardSession` path, transport
     se be-niyaz; naye nonce, counter 0 se — D3/D30). Board tag verify
     kare; OK mile to pairing kamyab.
  4. Kamyab tasdeeq par (ya window khatam hone par) board **paired** mark
     hota hai. Is ke baad HELLO mein sirf nonce milta hai — key kabhi nahi
     (serial par bhi nahi).
  5. Ghalti par: window ke andar GET_CAPS fail ho to app dobara koshish
     kar sakti hai (naya HELLO, naya session). Window khatam hone ke baad
     pairing band — dobara pairing ke liye KEY_RESET (§4.4) lazmi hai.
- `SET_PASSWORD` (§5.2) aur `KEY_RESET` (§4.4) wahi control commands hain
  — serial par bhi chalte hain (koi alag pairing command nahi).
- Wi-Fi pairing (§4.2) unchanged — dono raste ek hi key/session rules
  istemal karte hain; ek dafa paired board dono transports par paired hai.

### 10.6 App side — auto-detect + transport switch

- **Auto-detect:** candidate ports par HELO probe (500 ms timeout) —
  pehla port jis se valid HELLO (92 B, §2.2) aaye, wahi RSH board hai.
  Ghalat port (doosra USB-serial device, khaali port) khamoshi se chhor
  diya jata hai — koi exception bahar nahi aati, sirf null milta hai.
  (`SerialDeviceDetector.DetectAsync` / `DetectOnSystemPortsAsync`.)
- **Transport switch** (`AppServices`): `SwitchToSerialAsync(portName)` /
  `SwitchToWifi()` / `DetectAndSwitchToSerialAsync()` — engine dobara
  banta hai (naye source ke saath), HELO serial se bheja jata hai,
  control transport serial par chala jata hai (`ActiveControlTransport`).
  Switch par purana transport band hota hai (port close + dispose).
- **Koi engine duplication nahi:** `SerialWheelSource` wahi interfaces
  implement karta hai jo `UdpWheelSource` karta hai (`ISampleSource` /
  `IV2SampleSource` / `IStatusSource` / `IWheelSource`) — WheelEngine ko
  farq nahi parta ke packet UDP se aaya ya USB-serial se. Link SHARED
  hota hai (control transport bhi usi link par), is liye source by default
  link ka malik nahi hota (`ownsLink: false` — `Stop()` link ko band nahi
  karta).
- **Ping SLO wired:** E2E p99 ≤ 5 ms (§9.2 — Wi-Fi ≤ 10 ms, wired ≤ 5 ms);
  wahi `NtpPingClient`, wahi 34 B frame, sirf transport alag hai
  (`SerialPingTransport`).

### 10.7 Verification — sandbox mein kya hua, kya baaqi hai

- **C# (sandbox):** `PhaseGWorkstream1Tests` — 17 tests, sab pass: codec
  canonical vectors + round-trip + error rules, decoder (byte-at-a-time /
  chunked / garbage resync / bad CRC), SerialLink loopback (HELO/HELLO,
  control GET_CAPS, ping echo, timeout, busy-throw, bad-HELLO counting,
  unsolicited/dropped), transports, SerialWheelSource (v2 stream + status
  + HELLO slot), detector (live port milta hai, dead port skip). Asal USB
  port / board ki zaroorat nahi — in-memory `LoopbackSerialPort` do
  `ISerialPort` ko jorta hai (rig pty jaisa, lekin process ke andar).
- **C++ (sandbox):** host tests §22 — framing build/parse, canonical
  vectors byte-exact, decoder byte-at-a-time/chunked/garbage; Python
  vectors (§10.1) 3-language match.
- **Firmware (sandbox):** `pio run` SUCCESS — `serial_link` + `net_serial_*`
  + `rsh.ino` emission wiring; real board par stream rate verify nahi hua.
- **Rig scenario nahi hai:** Virtual Test Rig ka harness UDP-based hai —
  serial ke liye koi rig scenario nahi banaya gaya (rig behaviour badle
  baghair). Sandbox verification = WS1 loopback tests + host tests (rig
  ke unit stage mein chalte hain).
- **Phase H A7 (real board / Windows PC) par verify hoga:** asal CP2102
  port par 921600 baud link, 200 Hz serial stream rate + jitter, cable
  unplug/replug recovery, DTR/RTS no-reset behaviour, Windows COM port
  auto-detect, wired pairing end-to-end (pehli dafa key exchange USB se).

## Appendix A — v1 legacy packet (reference, unchanged)

v1 data packet (18 B): `0xA5 | 0x01 | seq u16 | ts u32 | steer u16 |
accel u16 | brake u16 | buttons u8 | flags u8 | CRC16`. Golden:
`a5 01 34 12 04 03 02 01 fc 08 e8 03 d0 07 05 01 75 bd`
(CRC re-verified, `protocol_v2_vectors.py`). Legacy mode: version byte
`0x01` dekhte hi app legacy path leti hai; v1 app ko v2.1 board v1 packets
bhejta hai (HELO 4-byte, §2.1).
