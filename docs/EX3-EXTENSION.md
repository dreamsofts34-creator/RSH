# RSH Protocol v2.2 — Button Extension Words (Ex2/Ex3) — DESIGN DOC ONLY

**Status:** DESIGN ONLY — Phase E WS5 deliverable. **Koi code implement
nahi karta.** Ye doc v2.1 ke button gap (§1.1 N1) ko v2.2 mein band karne
ka design hai. **Authority:** `docs/PROTOCOL-V2.1.md` (§1.1 N1, §1.2,
§3.4) — jahan farq ho, v2.1 jeetta hai jab tak v2.2 contract na likha
jaye. Faisle yahan **PROPOSED** hain — coordinator approval ke baghair
in par code nahi likha jayega.

**Versions:** firmware v4.0.0 / app v2.0.0 / protocol **v2.1** (maujooda).
Ye doc **protocol v2.2** (proposed) ke baare mein hai.

## 0. Masla (N1 ka khulasa)

v2.1 wire par sirf buttons **0–63** aate hain:

| Word | Wire jagah | Buttons | Rate |
|---|---|---|---|
| W0 `buttons` | stream packet (§1.1), bytes 14–17 | 0–31 | 200 Hz |
| W1 `btnsEx` | status packet (§1.2), bytes 28–31 | 32–63 | 1 Hz |

Config ceiling **nBtn ≤ 128** hai (§0, D6), lekin wire slot 64 se oopar
nahi hai — is liye SET_CFG aisi config (nBtn 65–128) ko `UNSUPPORTED`
(6) se reject karta hai (§1.1 N1). vJoy taraf Ex2 (65–96) aur Ex3
(97–128) ka **koi wire source nahi** — PC un bits ko kabhi fabricate
nahi karta (Phase D WS7 tests yehi check karte hain: oopar wale bits
hamesha 0).

**Ahem nuqta:** Ex3 (97–128) akela nahi aa sakta — 64–96 (Ex2) ke
baghair 97–128 ka koi matlab nahi (button numbering musalsal hai). Is
liye ye doc **dono** extension words ka design karta hai (W2 = 64–95,
W3 = 96–127), aur Ex3 us ka doosra hissa hai.

**Numbering note:** is doc mein **wire numbering (0-based)** istemal
hoti hai — W2 = wire buttons 64–95, W3 = wire buttons 96–127. vJoy
numbering **1-based** hai: vJoy Ex2 (buttons 65–96) = wire 64–95,
vJoy Ex3 (buttons 97–128) = wire 96–127. Dono ek hi cheez hain, sirf
gin-ti ka farq hai — code mein hamesha wire numbering se kaam karo
aur vJoy mapping par +1 lagao (Ex1 ka bhi yahi tareeqa hai).

**Doosra nuqta:** ye sirf wire ka masla nahi hai — input hardware ka
bhi hai. Maujooda board par ~15 digital inputs hain; 128 buttons ke
liye button matrix (16 per matrix record, §3.6) ya expanders
(MCP23017/PCF8574, kind codes 8/9, §3.4.1) chahiye. Hardware mapping
is doc ka topic nahi — Phase F/G ka hai. Ye doc sirf wire format +
negotiation + migration design karta hai.

## 1. Proposed wire format (v2.2)

### 1.1 Status packet badhta hai (stream nahi)

**Faisla (proposed):** naye words **status packet** (1 Hz) mein aate
hain, stream packet (200 Hz) mein nahi. Wajah:

- Stream hot path hai — har byte airtime + parse cost hai; buttons
  64+ aam tor par hamesha nahi badalte, 1 Hz kaafi hai (Ex1 ka bhi
  yahi tareeqa hai — btnsEx status mein hai, §1.2).
- Status packet pehle hi length se pehchana jata hai (34 vs 38 B) —
  nayi lengths usi pattern mein aati hain, parser ka structure nahi
  badalta.
- CRC-last invariant (§0, D2) barqarar rehta hai — naye words
  `seqGap`/`txFail`/CRC se **pehle** aate hain.

Nayi lengths (parser ko ye set accept karna hoga, aur koi nahi):

| Length | Matlab | Layout (base fields 0–27 waisi ki waisi) |
|---|---|---|
| 34 B | base (nBtn ≤ 32) | seqGap 28–29, txFail 30–31, CRC 32–33 |
| 38 B | + W1 btnsEx (33–64) | btnsEx 28–31, seqGap 32–33, txFail 34–35, CRC 36–37 |
| 42 B | + W2 btnsEx2 (65–96) | btnsEx 28–31, btnsEx2 32–35, seqGap 36–37, txFail 38–39, CRC 40–41 |
| 46 B | + W3 btnsEx3 (97–128) | btnsEx 28–31, btnsEx2 32–35, btnsEx3 36–39, seqGap 40–41, txFail 42–43, CRC 44–45 |

Bit rule (W0/W1 jaisi): word `Wk` mein bit *(n − 32k)* = button *n*.
Misal: button 100 → W3 (96–127) bit 4.

**Reject rule (proposed):** length 34/38/42/46 ke ilawa → `BAD_LENGTH`
(drop + count). Reserved bytes / flags rules waisi ki waisi (§1.4).

### 1.2 Stream packet nahi badalta

Stream packet (§1.1) **waisa hi rehta hai** — `buttons` u32 (0–31),
koi trailing extension word nahi. 200 Hz par har extra word har
packet mein 2–4 B barhata, jab ke faida sirf un boards ko hai jin par
64+ buttons configured hain. v2.2 mein stream ka version byte `0x22`
hota hai (neeche §2), lekin layout wahi hai.

### 1.3 GET_CAPS (proposed)

| Field | v2.1 | v2.2 (proposed) |
|---|---|---|
| `protoMinor` (§3.4) | `1` | `2` |
| `nBtnMax` (§3.4) | `64` (wire) | `128` (wire — chaaron words) |
| `capsFlags` bit4 (§3.4) | reserved `0` | `btnExt` — 1 = board Ex2/Ex3 words bhej sakta hai |

`capsFlags` bits 5–7 reserved `0` rehte hain.

### 1.4 SET_CFG validation (proposed)

- nBtn 65–128 **ab UNSUPPORTED nahi** — accept hota hai (jab board
  v2.2 ho). Validation waisi ki waisi (§3.6): button records ≤ 128,
  matrix record = 16 buttons, duplicate GPIO → `PIN_CONFLICT`.
- nBtn > 128 → ab bhi `PIN_CONFLICT` (parser bound, §0).
- Config blob format (§3.6) **nahi badalta** — records pehle hi 128
  buttons gin sakte hain; sirf validator ki N1 wali `> 64 →
  UNSUPPORTED` shart khatam hoti hai.

## 2. Version negotiation (proposed — sab se ahem faisla)

### 2.1 Masla

Board ko kaise pata chale ke PC 42/46 B status samajhta hai? HELO
(§2.1) mein sirf proto byte (`0x02` = v2.x) hai — **minor version
nahi**. Purani app (v2.1) ko 46 B status mile to wo `BAD_LENGTH` se
drop karegi — stream chalta rahega, lekin Ex1 (33–64) bhi gayab ho
jayega (wo bhi usi packet mein hai). Is liye board **andha** extended
status nahi bhej sakta.

### 2.2 Proposed: HELO mein app minor version (7th byte)

HELO (§2.1) mein ek optional 7th byte jodein:

| HELO length | Matlab |
|---|---|
| 5 B (aaj ki v2 app) | proto `0x02`, sessionFlag nahi — app minor **unknown** (2.1 mano) |
| 6 B (aaj ki v2 app) | proto `0x02` + sessionFlag — app minor **unknown** (2.1 mano) |
| 7 B (v2.2 app) | proto `0x02` + sessionFlag + **appMinor** (`2` = v2.2) |

Rules (proposed):

- 7th byte maujood ho aur `0`/`1`/`2` ke ilawa ho → HELO ignore
  (proto/sessionFlag wali sakhti jaisi, §2.4).
- Board extended status **sirf tab** bhejta hai jab (a) nBtn > 64
  configured ho **aur** (b) aakhri HELO se appMinor ≥ 2 maloom ho.
  Warna board 34/38 B status bhejta hai (jaisa aaj hai).
- HELLO reply ka `protoMinor` board ki version batata hai (2.2);
  HELO ka 7th byte app ki version batata hai. Dono taraf se
  negotiation hoti hai — koi ek taraf faisla nahi karta.
- Legacy v1 app (HELO proto `0x01`) par koi asar nahi — usay v1
  packets milte rehte hain.

### 2.3 Alternative jo reject kiya gaya

- **GET_CAPS se gating:** app GET_CAPS ke baad board ko "main v2.2
  hoon" bataye (koi naya command ya flag). Reject — GET_CAPS
  HMAC-signed hai aur har HELO ke baad nahi hota; board ko har 1 Hz
  status se pehle ye maloom hona chahiye ke PC kaun si version hai.
  HELO (1 Hz, har packet mein) hi sahi jagah hai.
- **Hamesha extended status bhejo, purani app khud sambhal le:**
  Reject — §2.1 ka masla (purani app Ex1 bhi kho deti hai). Silent
  regression hai.
- **Stream mein trailing words:** Reject — §1.2 (hot path cost).

## 3. PC-side changes (proposed, implement nahi karna — sirf list)

1. `StatusPacketV2` (C#) + `rsh_v2_parse_status` / `rsh_v2_build_status`
   (C++): lengths 42/46 accept karo, `btnsEx2`/`btnsEx3` fields jodein.
2. `WheelPacketV2` / engine: `LatestButtonsEx2` / `LatestButtonsEx3`
   (Ex1 ke `LatestButtonsEx` jaisa pattern, Phase D WS7).
3. vJoy output: Ex2 (65–96) → vJoy buttons 65–96, Ex3 (97–128) →
   vJoy buttons 97–128 (vJoy 128 buttons support karta hai — wahi
   mapping jo Ex1 ke liye hai).
4. `ProtocolV2.CheckButtonCount`: 65–128 ab `Ok` (v2.2 board par);
   v2.1 board par `Unsupported` rehta hai — faisla board version
   (GET_CAPS `protoMinor`) se hota hai, hard-code se nahi.
5. HELO sender (C# `HelloClient`): 7-byte HELO bhejo (appMinor = 2).
6. Golden vectors: `tests-host/protocol_v2_vectors.py` mein 42/46 B
   status vectors (positive + negative: ghalat length, CRC, W2/W3
   bits), 3-language match gate (T1/T2/T3) dobara green hona lazmi hai.

## 4. Migration path (proposed)

| Surat-e-haal | Kya hota hai |
|---|---|
| Purani app (2.1) + naya board (2.2) | Board 6-byte HELO dekh kar appMinor unknown manta hai → 34/38 B status. nBtn 65+ configure karne ki koshish kare to board **accept** karta hai (v2.2) lekin status 38 B par rehta hai — Ex2/Ex3 buttons nazar nahi aate (UI mein "app update karo" warning dikhao — Wave 2). Koi crash / silent Ex1 loss nahi. |
| Nayi app (2.2) + purana board (2.1) | Board HELLO `protoMinor = 1` bhejta hai; app 7-byte HELO bhejti hai (board ignore karta hai — 7th byte parha nahi jata, §2.4 jaisi leniency). SET_CFG 65+ → board `UNSUPPORTED` (v2.1 N1) — app wahi dikhaye jo aaj dikhati hai. |
| Nayi app + naya board | 7-byte HELO → board extended status bhejta hai (sirf jab nBtn > 64 ho). Full 128 buttons. |
| Config pehle se 65+ ho aur app downgrade ho jaye | Board ki stored config mein nBtn 65+ hai; purani app ko 38 B status milta hai (Ex2/Ex3 chhupte hain, Ex1 rehta hai). Config kharab nahi hoti — nayi app wapas aate hi full buttons wapas. |

**Upgrade order (R-M10 jaisa):** pehle app, phir firmware — v2.1 ka
usool barqarar hai.

## 5. Proposed decisions (coordinator approval ke liye)

| # | Faisla | Status |
|---|---|---|
| D36 | v2.2 mein W2 (64–95) + W3 (96–127) status packet mein aate hain; lengths 42/46 B; stream nahi badalta | PROPOSED |
| D37 | HELO mein optional 7th byte = appMinor; board extended status sirf appMinor ≥ 2 par bhejta hai | PROPOSED |
| D38 | GET_CAPS: `protoMinor = 2`, `nBtnMax = 128`, `capsFlags` bit4 = `btnExt` | PROPOSED |
| D39 | SET_CFG N1 shart khatam (65–128 accept, v2.2 board par); blob format nahi badalta | PROPOSED |
| D40 | CRC-last invariant + reject rules barqarar; 42/46 B ke ilawa length → BAD_LENGTH | PROPOSED |

## 6. Khula sawaal (is doc mein faisla nahi)

- **Q1 — Input hardware:** 97–128 buttons ke liye kaun se expanders /
  matrix? Pin budget + GET_CAPS pin table (40 entries ki had, §3.4)
  par asar — Phase F/G hardware work. Wire design is par munhasir
  nahi hai.
- **Q2 — vJoy Ex3 test:** vJoy 128 buttons ka rig test (virtual
  wheel 128 buttons) — rig workstream ka kaam hai, is doc ka nahi.
- **Q3 — Status rate:** 128 buttons par 1 Hz kaafi hai ya Ex2/Ex3 ko
  10 Hz chahiye? Aaj Ex1 1 Hz par hai aur koi shikayat nahi — 1 Hz
  rakho, jab tak koi measurement na kahe ke kam hai.

---

*Doc type: design only. Is mein likha koi bhi rule code nahi hai —
implementation v2.2 workstream (Phase F/G) mein hogi, is doc aur
coordinator approval ke mutabiq. v2.1 contract
(`docs/PROTOCOL-V2.1.md`) is doc se nahi badalta.*
