# RSH — Threat Model (STRIDE) (WP4)

**Status:** DESIGN DOC ONLY — Phase A, WP4 deliverable. Koi code implement nahi karta.
**Sathi document:** `KEY-LIFECYCLE.md` (key lifecycle design — is threat model ke
mitigations ka aksar hawala us doc se hai).
**Authority:** RSH-PLAN-v2.3.md §2, §5, §6 (Security row), §7 (MoSCoW).
**Standards (Plan §6):** OWASP ISVS v1.0 (2025) + NIST IR 8259A + ETSI EN 303 645.
(OWASP IoT Top 10 2018 outdated baseline nahi hai.)
**Versions:** firmware v4.0.0, app v2.0.0, protocol v2.1 (D27).
**Product naam:** RSH.

## 0. Tareeqa aur scope

- **Method:** STRIDE — Spoofing, Tampering, Repudiation, Information Disclosure,
  Denial of Service, Elevation of Privilege. Har threat ke liye: attack, asar,
  mitigation + **kis phase mein fix hoga**, aur residual risk.
- **Threat IDs:** `TM-` prefix — audit findings `T1–T15` se takrao na ho, is liye
  alag numbering hai.
- **Assumptions (ye threat model in par khara hai):**
  1. Koi factory / cloud / internet service nahi — board aur PC ke darmiyan
     sirf local Wi-Fi SoftAP hai (aur Phase G mein wired USB-serial).
  2. Hamlaawar board ki Wi-Fi range mein aa sakta hai; internet se hamla nahi
     kar sakta (board internet par nahi hota).
  3. User ka PC aur Windows account user ke control mein hai (PC compromise
     hone par DPAPI ki hadood §3 mein hain).
  4. Physical access wala hamlaawar board ko haath mein le sakta hai — BOOT-hold
     rasta is liye physical maujoodgi mangta hai, aur physical attacker ke
     khilaf koi software daawa nahi kiya jata (dekho §5, Out of scope).
  5. DTLS evaluate kiya gaya aur **defer** kiya gaya — local AP + HMAC v2 design
     hai, oversight nahi (Plan §2). Is liye transport encryption ka bojh AP +
     HMAC + pairing window par hai; har threat mein ye baat yaad rakho.
- **Assets:** (1) device key (control channel), (2) Wi-Fi/OTA password,
  (3) pin map / config, (4) firmware image, (5) PC profiles + calibration,
  (6) Secure Boot / image-signing keys (offline).

## 1. Trust boundaries aur exposed surfaces

```
[ Internet ] --(koi rasta nahi)--> [ Board SoftAP ] <-- Wi-Fi --> [ PC app ]
                                        |  UDP 4212 (stream + control)
                                        |  WebOTA (HTTP, Phase D: auth)
                                        |  HELLO / pairing (60 s window)
[ Physical attacker ] -- BOOT-hold --> [ Board (eFuse, flash, NVS) ]
[ PC user / malware ] -- DPAPI -------> [ %AppData%\RSH : key, profiles ]
```

**Teen exposed surfaces** (task ke mutabiq — har ek ka apna section neeche hai):

| # | Surface | Kahan |
|---|---|---|
| A | Wi-Fi SoftAP + UDP (stream + control + HELLO) | §2 |
| B | WebOTA (HTTP) | §3 |
| C | PC app (credentials, settings, keys) | §4 |

## 2. Surface A — Wi-Fi SoftAP + UDP

Board ka SoftAP (default IP 192.168.4.1 configurable, D33; mDNS `rsh.local`
pehle, fallback IP) + UDP port 4212. Stream packets (200 Hz) aur control frames
(HMAC-signed) isi par chalte hain.

### TM-W01 — Spoofing: nakli board (evil twin / rogue AP)

- **Attack:** Hamlaawar board jaisa SSID wala AP kholta hai; app us se jur kar
  HELLO bhejti hai aur key / credentials us nakli board ko de deti hai. Ya
  hamlaawar board ban kar PC ko ghalat GET_CAPS / stream bhejta hai.
- **Asar:** Zyada — key leak hone par asal board ka control hamlaawar ke paas.
- **Mitigation:**
  - Pairing sirf 60 s window + pehli HELLO mein (WP4 design, `KEY-LIFECYCLE.md` §3).
  - Per-device random AP password (NIST 8259A) — hamlaawar ko asal AP ka
    password maloom nahi, is liye user ke PC ko asal AP se hatana mushkil hai.
  - App ko pehli pairing ke baad board ki pehchan (boardMac, D21 — Profile v2)
    se bandhna chahiye; naye board par warning + dobara pairing.
    (Exact binding WP3/Phase C mein — OPEN, `KEY-LIFECYCLE.md` OPEN-7.)
- **Fix phase:** WP4 (design — ye doc) + WP3 (HELLO/board identity) + Phase C (app).
- **Residual risk:** Medium. Pehli pairing ke waqt evil twin mumkin hai agar
  hamlaawar usi 60 s mein range mein ho — is ka koi cryptographic hal nahi
  jab tak board ki pehchan pehle se maloom na ho. Mitigation: pairing sirf
  apne ghar/room mein karo (USER-GUIDE, Phase H).

### TM-W02 — Information Disclosure: pehli HELLO mein key transfer sniff hona

- **Attack:** Hamlaawar pairing window ke dauran Wi-Fi traffic sniff karta hai
  aur pehle HELLO mein bheji gayi device key + Wi-Fi/OTA password pakar leta hai.
- **Asar:** Zyada — key milne par tamam control commands sign kar sakta hai.
- **Mitigation:**
  - Disclosure ek dafa, 60 s window, local AP par (WP4 design).
  - Link ki hifazat ka darja pairing bootstrap par munhasir hai —
    `KEY-LIFECYCLE.md` OPEN-6: WP3 ko bootstrap decide karna hai. Agar pehli
    pairing ke liye AP khula ho to us window mein sniffing ka koi cryptographic
    rok nahi — sirf waqt + proximity.
  - Key transfer ke baad key dobara kabhi wire par nahi aati.
- **Fix phase:** WP3 (bootstrap faisla — pairing implement karne se pehle lazmi).
- **Residual risk:** Medium-High jab tak OPEN-6 band na ho. Ye is threat model
  ka sab se bara khula nuqta hai — is liye OPEN-6 ko WP3 ka pehla faisla hona
  chahiye. DTLS defer ka faisla is risk ko khatam nahi karta, sirf document karta hai.

### TM-W03 — Tampering / Replay: control frame badalna ya dobara bhejna

- **Attack:** Hamlaawar UDP control frame pakar kar dobara bhejta hai (replay —
  misal FACTORY_RESET dobara chalana), ya payload badal kar bhejta hai
  (misal SET_CFG mein ghalat pin map).
- **Asar:** Zyada — ghalat config, factory reset, ya board ka ghalat istemal.
- **Mitigation:**
  - HMAC over `nonce ‖ counter ‖ cmd ‖ payload`, per-device key (D3, Plan §1).
    Tag ghalat ho to board frame reject karta hai (error BAD_TAG=1).
  - Counter per-session monotonic hai; naya nonce = counter 0 (D3/D30).
    Purana counter dobara aaye to REPLAY=2 reject.
  - Retry mein reqId wahi, counter naya; board (reqId, counter) pair se
    duplicate pehchanta hai (D3).
  - Stream packets par CRC16-CCITT (D2) — tampering detect hoti hai, lekin
    stream ka HMAC nahi hai (dekho TM-W04).
- **Fix phase:** WP3 (byte layouts + negative vectors, T2) + Phase D (firmware F14).
- **Residual risk:** Low (control channel). Baqi risk: nonce/counter ka exact
  session handling WP3 vectors se verify hoga (T1 parser, teeno languages).

### TM-W04 — Tampering: stream packets (input data) badalna

- **Attack:** Hamlaawar stream packet badal kar PC ko ghalat steering / pedal
  data bhejta hai (game mein ghalat input), ya packet drop kar ke loss barhata hai.
- **Asar:** Medium — game mein ghalat input; koi config ya key leak nahi.
- **Mitigation:**
  - CRC16-CCITT har stream packet par (D2) — badla hua packet drop hota hai.
  - Status packet mein seqGap + txFail counters (D5) — loss nazar aata hai.
  - Stream ka HMAC **nahi** hai (design faisla — 200 Hz par HMAC ka kharcha
    aur zarorat WP3 mein dobara dekhna; is doc mein faisla nahi badla gaya).
- **Fix phase:** WP3 (vectors) + Phase D (F14). HMAC-on-stream par dobara
  ghaur WP3 ka OPEN item hai (agar WP3 chahe to note kare — is doc mein
  faisla nahi).
- **Residual risk:** Medium-Low. Attacker sirf DoS / ghalat input kar sakta hai
  (packet drop = loss), control nahi le sakta.

### TM-W05 — Denial of Service: restart / flood se board girana

- **Attack:** (a) WebOTA restart endpoint ka ghalat istemal (F1 — §3 dekho);
  (b) UDP flood se board ki loop / heap khatam karna; (c) Wi-Fi deauth se PC ko
  AP se girana; (d) ghalat / bare frames se parser crash karana.
- **Asar:** Zyada (race ke darmiyan wheel band) — Medium (deauth sirf local).
- **Mitigation:**
  - (a) F1 auth fix — har endpoint par `authenticate()` lazmi (D24, Phase D).
  - (b) Heap threshold restart rule (D17: largest free block < 8 KB lagatar
    10 cycles, ya 5 min mein 20% girawat) + full-loop timing napna (D15/F11).
  - (c) Deauth ka koi software hal nahi — local AP ki fitri kamzori; residual.
  - (d) Strict length checks + bounds (D6: nAxes ≤ 8, nBtn ≤ 128, nEnc ≤ 4;
    zyada ho to BAD_FRAME reject); parser host tests + fuzz (Phase D, T4).
  - Boot-loop guard: 3 lagatar fail ⇒ last-known-good, phir factory (D29).
- **Fix phase:** Phase D (F1, F10, F11, T4) + WP3 (bounds/vectors).
- **Residual risk:** Medium. Deauth aur RF jamming ka hal nahi hai; wired
  transport (Phase G) is ka asal jawab hai.

### TM-W06 — Spoofing: doosra device AP par aa kar control karna

- **Attack:** Koi doosra device (misal ghar ka koi shakhs ya mehman) board ke AP
  se jur jata hai aur control commands bhejta hai.
- **Asar:** Medium — AP join karna hi kaafi nahi (HMAC key chahiye), lekin
  stream dekh sakta hai aur DoS kar sakta hai.
- **Mitigation:**
  - HMAC key ke bagair control frame reject (TM-W03).
  - Per-device random AP password — sirf wohi jure jo password janta ho.
  - AP par client limit / sirf ek PC ki policy WP3/Phase D mein decide hogi
    (OPEN — is doc mein faisla nahi).
- **Fix phase:** WP3/Phase D.
- **Residual risk:** Low (control), Medium (sniffing/DoS — TM-W02/W05 dekho).

### TM-W07 — Information Disclosure: flash / NVS dump se key nikalna

- **Attack:** Hamlaawar board haath mein le kar flash dump karta hai (USB-serial /
  flash reader) aur encrypted NVS se key nikalne ki koshish karta hai.
- **Asar:** Zyada (agar kamyab ho to poora control).
- **Mitigation:**
  - Key encrypted NVS mein hai (WP4 design).
  - Secure Boot v2 RSA-3072 (eFuse digest) — sirf signed image boot hoti hai;
    hamlaawar apni image flash kar ke key nahi nikal sakta (jab tak Secure Boot
    enable ho — enablement Phase D ka kaam hai).
  - Flash encryption enable karna hai ya nahi — OPEN-3 (`KEY-LIFECYCLE.md` §9):
    is ke bagair flash dump se NVS cipher parha ja sakta hai (key nikalna
    phir bhi NVS encryption key mangta hai, lekin attack surface barhta hai).
- **Fix phase:** Phase D (Secure Boot enablement + flash encryption faisla).
- **Residual risk:** Medium jab tak OPEN-3 band na ho; physical attacker ke
  khilaf koi bhi software daawa mehdood hai (dekho §5).

### TM-W08 — Elevation of Privilege: unauthenticated command se config badalna

- **Attack:** Hamlaawar bina key ke SET_CFG / FACTORY_RESET / REBOOT bhej kar
  board ki config badal deta hai ya board reboot kar deta hai.
- **Asar:** Zyada.
- **Mitigation:**
  - Tamam control commands HMAC-signed honi lazmi hain; bina valid tag ke
    board command execute nahi karta (WP3 + Phase D).
  - FACTORY_RESET (0x08) key ko wipe nahi karta, lekin us ke liye bhi valid
    HMAC lazmi hai (`KEY-LIFECYCLE.md` §5).
  - KEY_RESET sirf physical BOOT-hold-armed state mein (WP4 design).
- **Fix phase:** WP3 (command auth table) + Phase D (firmware).
- **Residual risk:** Low (agar har command par HMAC enforce ho — WP3 ko har
  command ke liye auth lazmi likhna hoga; koi command "public" nahi honi chahiye
  siwa HELLO/GET_CAPS ke — aur GET_CAPS mein key kabhi nahi).

## 3. Surface B — WebOTA (HTTP)

WebOTA rehta hai (band nahi hota — D24), lekin **auth fix ke saath (F1)**.
Phase D mein fix hoga. WebOTA HTTP par hai (local AP par, koi TLS nahi —
TLS/DTLS defer ka hissa; certificate management per-device AP par feasible nahi).

### TM-O01 — Auth Bypass (F1): bina password ke firmware upload / restart — SAB SE BARA

- **Attack:** Audit finding F1 — WebOTA ke kuch endpoints par auth nahi thi.
  Hamlaawar AP par aa kar (ya AP password jan kar) bina auth ke firmware
  upload karta hai ya restart endpoint se board ko bar bar restart karta hai.
- **Asar:** Critical — bina auth ke koi bhi binary flash kar sakta hai; board
  poori tarah hamlaawar ka ho jata hai.
- **Mitigation:**
  - **Har endpoint par `authenticate()` lazmi** (D24) — koi endpoint bina auth
    ke nahi, restart samet. Ye F1 ka fix hai.
  - Upload se pehle image signature verify (Ed25519/ECDSA P-256, §9
    `KEY-LIFECYCLE.md`) — auth ke baad bhi ghalat image reject.
  - Anti-downgrade — purani version reject.
  - Signed OTA Must hai (MoSCoW, Plan §7) — auth aur signature dono lazmi hain;
    ek doosre ka badal nahi.
- **Fix phase:** **Phase D (F1)** — Phase A mein sirf design (ye doc).
- **Residual risk:** Low (fix ke baad). Jab tak F1 fix na ho, WebOTA ko
  istemal ke waqt AP password hi waahid rukawat hai — is liye F1 "foran karne
  wale" kaam mein hai (Plan §5: F1+A1 security).

### TM-O02 — Tampering: beech mein badli hui firmware image

- **Attack:** Hamlaawar OTA upload ke dauran image badal deta hai, ya adhoori /
  kharab image upload karta hai.
- **Asar:** Zyada — brick ya compromised firmware.
- **Mitigation:**
  - Image signature verify **before apply** (TM-O01).
  - Size / magic check (F9: PROGMEM + size/magic check).
  - Upload validation (A1 — Phase B, app side).
  - Brick recovery doc + test (Phase D, R-M12) — kharab flash ke baad wapas
    aane ka rasta.
- **Fix phase:** Phase D (F9) + Phase B (A1) + Phase H (release checklist).
- **Residual risk:** Low.

### TM-O03 — Information Disclosure / Brute Force: OTA password guess karna

- **Attack:** Hamlaawar WebOTA login par password guess karta hai (brute force /
  dictionary). AP password aur OTA password ka rishta OPEN-13 hai — agar dono
  ek hi hain to ek guess dono khol deta hai.
- **Asar:** Zyada (kamyab guess = TM-O01).
- **Mitigation:**
  - Random per-device password, MAC se derive nahi (WP4 design).
  - Forced change pehli setup par (NIST 8259A).
  - Rate limiting / lockout ka faisla WP3/Phase D mein hona hai — **OPEN**
    (is doc mein faisla nahi; recommendation: ghalat koshishon par delay /
    lockout — WP3 band kare).
- **Fix phase:** WP3 (OPEN-13/OPEN-14) + Phase D.
- **Residual risk:** Medium jab tak rate limiting ka faisla na ho. Random
  lamba password brute force ko mushkil banata hai, lekin rate limit ke bagair
  online guessing mumkin rehta hai.

### TM-O04 — Repudiation / CSRF: browser se WebOTA trigger karna

- **Attack:** User ke PC par khula browser (jo board ke AP par hai) kisi
  malicious page se WebOTA endpoint ko request bhejta hai (CSRF) — misal
  restart ya upload trigger.
- **Asar:** Medium.
- **Mitigation:**
  - Har endpoint par auth (TM-O01) — browser ke paas password nahi hota to
    request fail hoti hai.
  - Upload ke liye POST + auth token ka tareeqa WP3/Phase D mein define hoga
    (OPEN — is doc mein faisla nahi).
- **Fix phase:** Phase D.
- **Residual risk:** Low (auth ke baad).

## 4. Surface C — PC app (credentials, settings, keys)

App: C# / .NET 10 / WPF v2.0.0, Windows 11 x64. Credentials DPAPI se,
profiles `%AppData%\RSH` mein.

### TM-P01 — Information Disclosure: DPAPI key chori (malware / doosra user)

- **Attack:** PC par malware (usi Windows user se chal raha) DPAPI decrypt kar
  ke device key nikal leta hai; ya key plain text mein log / crash dump /
  settings export mein leak hoti hai.
- **Asar:** Zyada — key milne par board ka poora control.
- **Mitigation:**
  - DPAPI CurrentUser scope (WP4 design) — disk nikaal kar parhne se hifazat.
  - Key kabhi log / crash dump / telemetry / export / UI mein nahi
    (`KEY-LIFECYCLE.md` §4; telemetry default OFF, D31; crash-dump redact).
  - Key ka koi export / backup / cloud sync nahi.
  - **Limit (honest):** DPAPI user-bound hai, app-bound nahi — usi user ka
    malware key khol sakta hai. Is ka hal app ke andar nahi; PC ki hifazat
    (Windows Update, antivirus, admin rights na dena) user ki zimmedari hai —
    USER-GUIDE mein likhna hai (Phase H).
- **Fix phase:** Phase C/E (implementation) + Phase H (docs).
- **Residual risk:** Medium. PC compromise = key compromise — is ko khatam
  karne ka koi design nahi hai; KEY_RESET (§5 `KEY-LIFECYCLE.md`) se key
  badal kar asar khatam kiya ja sakta hai (agar user ko pata chal jaye).

### TM-P02 — Tampering: profile / settings file badalna

- **Attack:** Hamlaawar ya ghalat import profile file badal deta hai (ghalat
  calibration, ghalat pin map, ghalat vJoy mapping) — ya app khud ghalat
  profile migrate kar deti hai (v1 → v2).
- **Asar:** Medium — ghalat output (P1/P2 jaisi ghaltiyan), data loss.
- **Mitigation:**
  - Profile v2 + schemaVersion + migration (Phase C, A10).
  - 3-generation backup (`.bak`, `.bak1`, `.bak2`, rotate — R-M11) + Settings
    mein "Backup now" / "Restore"; rename migration se pehle poora AppData copy.
  - Import/Export async + size cap + version check (Phase E, A9).
  - Backup guard (A3 — Phase B).
- **Fix phase:** Phase B (A3) + Phase C (A10, backup) + Phase E (A9).
- **Residual risk:** Low.

### TM-P03 — Information Disclosure: credentials installer / logs / crash dump mein

- **Attack:** Installer, logs, ya crash dump mein key / password / Wi-Fi
  password leak hota hai; ya purana AppData folder (rename se pehle wala)
  mein credentials reh jate hain.
- **Asar:** Medium-Zyada.
- **Mitigation:**
  - Logging mein secrets kabhi nahi — log redaction Phase C/E mein.
  - Crash-dump redact + opt-in, default OFF (D31).
  - Rename migration (Phase E): user-visible strings + AppData migration —
    purana folder copy ke baad saaf karne ka rule Phase E mein define hoga
    (OPEN — is doc mein faisla nahi).
  - MSIX installer (D22) + OV certificate signing (D23) — installer ki
    asliyat ki tasdeeq.
- **Fix phase:** Phase C/E/H.
- **Residual risk:** Low.

### TM-P04 — Elevation of Privilege: malicious profile / firmware file se code execution

- **Attack:** User ko malicious profile ya firmware `.bin` di jati hai; app use
  khol kar ya flash kar ke code chala deti hai.
- **Asar:** Zyada.
- **Mitigation:**
  - Import par size cap + version check + validation (A9, Phase E).
  - Firmware upload se pehle size/magic check (F9) + signature verify
    (TM-O01/O02) — board ghalat image apply nahi karta.
  - vJoy DLL bundle kabhi nahi (D18) — sirf registry / ProgramFiles se resolve,
    na mile to download link dikhao.
  - SBOM + THIRD-PARTY-LICENSES.md har release ke saath (Phase H, R-M29).
- **Fix phase:** Phase B (A1), Phase D (F9), Phase E (A9), Phase H (SBOM).
- **Residual risk:** Low-Medium (user ko social engineering se ghalat file
  khulwana hamesha mumkin rehta hai — USER-GUIDE warning, Phase H).

### TM-P05 — Denial of Service / Repudiation: single-instance aur autostart

- **Attack:** App ki do instance chal kar vJoy / UDP port par larai karti hain;
  ya autostart ghalat state mein app khol deta hai.
- **Asar:** Low-Medium.
- **Mitigation:** Global mutex `Global\RSH.SingleInstance` (A4, Phase B);
  autostart verify (A5 — runtime test Phase D mein, R-M2).
- **Fix phase:** Phase B/D.
- **Residual risk:** Low.

## 5. Out of scope / jo threat model cover nahi karta

- **Physical attacker board haath mein le kar:** flash dump (TM-W07 — Secure
  Boot / flash encryption se mehdood), BOOT-hold se KEY_RESET (by design —
  physical maujoodgi = authority), eFuse / chip-level attacks (decapping,
  glitching) — in ke khilaf koi daawa nahi.
- **PC ka OS / Windows compromise:** DPAPI ki had (TM-P01) — OS level par hal hai.
- **RF jamming / Wi-Fi deauth:** koi software hal nahi (TM-W05) — jawab wired
  transport hai (Phase G).
- **Supply chain (NuGet / ESP32 libs / vJoy):** SBOM + pinning (Phase H) — is
  doc mein sirf hawala hai, tafseel nahi.
- **FFB motor / HID PID:** v2 se bahar hai (Plan §6) — is liye is threat model
  mein nahi.
- **Wired USB-serial transport:** spec TBD hai (D26, Phase G) — us ka threat
  model us spec ke saath likha jayega (`KEY-LIFECYCLE.md` OPEN-20).

## 6. Residual risk summary

| # | Threat | Baqi risk | Kyun baqi hai |
|---|---|---|---|
| TM-W02 | HELLO key sniffing | Medium-High | OPEN-6 (bootstrap) band hone tak |
| TM-W01 | Evil twin pehli pairing par | Medium | Pehli pehchan ka koi pehle se rasta nahi |
| TM-W05 | Deauth / RF DoS | Medium | Koi software hal nahi; wired Phase G mein |
| TM-W07 | Flash dump se key | Medium | OPEN-3 (flash encryption) band hone tak |
| TM-P01 | PC malware se DPAPI key | Medium | DPAPI user-bound hai |
| TM-O03 | OTA password brute force | Medium | Rate limiting ka faisla baqi hai |
| TM-W04 | Stream tampering | Medium-Low | Stream par HMAC nahi (design faisla) |
| TM-W03 | Control replay | Low | HMAC + counter (WP3 vectors se verify hoga) |
| TM-W08 | Unauthenticated command | Low | Har command par HMAC (WP3 enforce kare) |
| TM-O01 | WebOTA auth bypass F1 | Low (fix ke baad) | **Aaj Critical** — Phase D mein fix hoga |
| TM-O02 | Badli hui image | Low | Signature verify + F9 |
| TM-O04 | CSRF | Low | Har endpoint par auth |
| TM-P02 | Profile tampering | Low | Backup + validation |
| TM-P03 | Logs/dump mein leak | Low | Redaction + D31 |
| TM-P04 | Malicious file | Low-Medium | Validation + signature; user awareness baqi |
| TM-P05 | Do instance | Low | Mutex |

**Sab se bara aaj ka risk:** TM-O01 (F1) — bina auth ke firmware upload / restart.
Ye Phase A mein design se nahi, **Phase D mein F1 fix se** khatam hoga. Is liye
jab tak F1 fix na ho, board ko kabhi bharay / shared Wi-Fi par istemal mat karo
— sirf apne ghar ke AP par (USER-GUIDE, Phase H).

## 7. Phase-wise mitigation map

| Phase | Threats |
|---|---|
| WP3 (Phase A) | TM-W02 (OPEN-6 bootstrap), TM-W03, TM-W04, TM-W06, TM-W08, TM-O03 (OPEN-13/14), OPEN-1 (purani protocol doc update) |
| WP4 (Phase A — ye doc) | Design: TM-W01, TM-W02, TM-W03, TM-W07, TM-O01 (design), TM-P01 |
| Phase B | TM-O02 (A1 upload validation), TM-P02 (A3 backup guard), TM-P05 (A4 mutex, A5 autostart) |
| Phase C | TM-W01 (board binding), TM-P01/P03 (DPAPI implementation), TM-P02 (A10 migration + backup) |
| Phase D | **TM-O01 (F1 — sab se bara)**, TM-W03/W05/W07/W08 (F9/F10/F11/F14, Secure Boot, brick recovery), TM-O02/O03/O04 |
| Phase E | TM-P02 (A9 import), TM-P03 (rename migration), TM-P01 (key handling UI) |
| Phase G | TM-W05 (wired transport — deauth ka jawab), OPEN-20 |
| Phase H | TM-O01 (signed OTA release), TM-P01/P03/P04 (docs, SBOM, signing keys), OPEN-19 (compromise response) |

## 8. Traceability — Plan v2.3 ke security points

| Plan point | Is doc mein |
|---|---|
| STRIDE threat model (Plan §2) | Ye poora document |
| Wi-Fi SoftAP + UDP (spoofing/tampering) | §2, TM-W01–TM-W08 |
| WebOTA auth bypass F1 | §3, TM-O01 (fix Phase D) |
| PC app credentials disclosure | §4, TM-P01–TM-P05 |
| DoS (restart) | TM-W05, TM-O01 |
| Elevation | TM-W08, TM-P04 |
| ISVS v1.0 + NIST IR 8259A + ETSI EN 303 645 (§6) | §0 — standards |
| DTLS evaluate + defer | §0.5 |
| Signed OTA Must (MoSCoW §7) | TM-O01, TM-O02 |
| Secure Boot RSA-3072 + Ed25519/ECDSA P-256 | TM-W07, TM-O01/O02; tafseel `KEY-LIFECYCLE.md` §9 |

---

*Doc type: design only. Mitigations ke phase labels Plan v2.3 §5 phase table se
liye gaye hain — koi phase ya fix is doc se nahi badla gaya.*
