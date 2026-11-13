# RSH — Key Lifecycle Design (WP4)

**Status:** DESIGN DOC ONLY — Phase A, WP4 deliverable. Koi code implement nahi karta.
Ye document Plan v2.3 §2 aur §0 Decision Log (D3, D30) ko design mein dhalta hai.
**Authority:** RSH-PLAN-v2.3.md — jahan is document aur kisi purani doc mein farq ho,
plan jeetta hai. Khaas tor par: purani `PROTOCOL-V2.md` mein key derivation
(OTA password se hash) **superseded** hai — WP3 ko us file ko is design ke mutabiq
update karna hoga (dekho §10, OPEN-1).
**Versions:** firmware v4.0.0, app v2.0.0, protocol v2.1 (D27).
**Product naam:** RSH. Koi doosra product naam is document mein istemal nahi hota.

## 0. Bunyadi usool (Plan v2.3 §2 — faisle band hain)

1. **Koi factory nahi hai.** User ghar par khud flash karta hai — is liye koi
   factory-provisioned key, koi shared default password aur koi cloud account nahi.
2. **Har board ki apni key.** Key per-device, random, aur sirf us board ke liye valid.
3. **Key board khud banata hai** — pehle boot par, board ke hardware RNG se.
4. **Key ek dafa disclose hoti hai** — pehle HELLO mein, 60 s pairing window ke andar.
   Window ke baad board key dobara kabhi nahi deta.
5. **App key ko DPAPI se store karti hai.** Key kabhi plain text mein disk, log,
   UI ya crash dump mein nahi aati.
6. **MAC se derive kabhi nahi.** MAC public hai — us se key ya password banana
   mana hai (R-H10).
7. **Koi shared default password nahi** (NIST IR 8259A secure defaults).
8. **DTLS defer hai** — local AP + HMAC v2 design hai, oversight nahi.
   Is ka matlab: transport ki hifazat ka bojh AP isolation + HMAC + pairing
   window par hai (threat model: `THREAT-MODEL.md`, TM-W01–TM-W08).

## 1. Key kya hai, kis kaam aati hai

| Cheez | Tafseel |
|---|---|
| **Control-channel key** ("device key") | Per-device secret. Control frames ka HMAC isi se banta hai: HMAC over `nonce ‖ counter ‖ cmd ‖ payload` (§1, D3). Tag 16 byte (D3 frame layout). |
| **Wi-Fi / OTA password** | Per-device random password — AP join aur WebOTA auth ke liye (§7). Ye control-channel key **nahi** hai; dono alag secrets hain. |
| **Secure Boot key** | RSA-3072 key pair; sirf public digest eFuse mein (§8). |
| **Image signing key** | Ed25519 / ECDSA P-256 (§8) — offline, kabhi repo/git/image mein nahi. |

Key length (control-channel key, byte mein) plan mein define nahi — dekho OPEN-2 (§10).
Recommendation (faisla nahi): 32 byte (256-bit), HMAC-SHA256 ke liye.

## 2. Pehli flash / pehla boot — key kaise banti hai (a)

1. User firmware v4.0.0 board par flash karta hai (USB-serial; wired transport
   spec Phase G, D26 — is doc ka topic nahi).
2. **Pehle boot par** firmware encrypted NVS mein device key dhoondta hai.
3. **Key na mile to:**
   - Board hardware RNG se nayi random key generate karta hai.
   - Key ko **encrypted NVS** mein likhta hai — plain flash mein kabhi nahi.
   - Board khud ko **unpaired** mark karta hai aur **pairing window (60 s)** kholta hai.
   - Board AP up karta hai (boot fail = 15 s ke andar AP up na ho, D29).
4. **Key mil jaye to:** board paired state mein boot hota hai; koi nayi key nahi
   banti; pairing window nahi khulti; HELLO mein key kabhi nahi di jati.
5. Key generation sirf in haalaton mein dobara hoti hai: KEY_RESET (§5), ya
   NVS poori tarah erase ho kar dobara flash (§6). Routine reboot, firmware OTA
   update, ya FACTORY_RESET par key dobara **nahi** banti.

**Design notes / limits:**
- NVS encryption ka exact mechanism (flash encryption enablement samet) WP3 /
  Phase D mein define hoga (OPEN-3). Secure Boot ke bagair flash encryption ka
  matlab samajhna zaroori hai: agar flash encryption enable nahi, to flash dump
  se NVS parha ja sakta hai — ye residual risk hai (`THREAT-MODEL.md`, TM-W07).
- Pehle boot par RNG ki tayyari (entropy) aur key-generation failure ka
  behaviour (retry? boot rok do? error blink?) WP3 / Phase D mein define hoga
  (OPEN-4). Rule: **kamzor ya repeat key ke saath boot kabhi aage nahi barhega**
  — fail closed.
- Key generation ka waqt (AP up se pehle ya baad) aur pairing window ka exact
  start trigger (AP-up se 60 s) WP3 mein confirm hoga (OPEN-5).

## 3. Pairing flow — step by step (b)

Pairing = board aur PC app ke darmiyan pehli dafa key + Wi-Fi/OTA password ka
ek-dafa tabadla. Sirf unpaired board pairing karta hai, sirf 60 s window mein.

| Step | Kaun | Kya hota hai |
|---|---|---|
| 1 | Board | Pehla boot (§2): key generate, encrypted NVS mein save, unpaired state, 60 s pairing window khulti hai. |
| 2 | User / app | PC board ke SoftAP se connect hota hai. **Bootstrap masla:** pehli dafa AP password app ko abhi maloom nahi — ye password bhi isi pairing mein board se milna hai. Is circular dependency ka hal plan mein define nahi — **OPEN-6 (§10)**. WP3 ko decide karna hoga (misal: pehli pairing ke liye waqti tor par khula AP, ya koi aur bootstrap). Jab tak OPEN-6 band na ho, pairing implement nahi ho sakti. |
| 3 | App | App board ko HELLO bhejti hai, **session flag = naya session** (D30). |
| 4 | Board | Board HELLO reply deta hai: **naya nonce** + (sirf is pehli HELLO mein, sirf window ke andar) **device key** + **per-device Wi-Fi/OTA password**. |
| 5 | App | App key ko **foran DPAPI se store** karti hai (§4) aur password ko OS Wi-Fi profile mein save karti hai; password ko **ek dafa QR / label ke tor par dikhati hai** (§7) aur phir kabhi plain dikhati nahi. |
| 6 | App | App pairing ki tasdeeq karti hai: HMAC-signed GET_CAPS bhejti hai (naye nonce, counter 0 se — D3/D30). Board tag verify kare; OK mile to pairing kamyab. |
| 7 | Board | Kamyab tasdeeq par (ya window khatam hone par) board **paired** mark hota hai. Is ke baad HELLO mein sirf nonce milta hai — key kabhi nahi. |
| 8 | Ghalti par | Window ke andar GET_CAPS fail ho to app dobara koshish kar sakti hai (naya HELLO, naya session). Window khatam hone ke baad pairing band — dobara pairing ke liye KEY_RESET (§5) lazmi hai. |

**Transfer ka channel kya hai aur kyun mehfooz hai:**
- Channel board ka apna SoftAP hai — local link, koi internet, koi cloud nahi.
  Hamlaawar ko (a) board ki Wi-Fi range mein hona, (b) AP se jura hona, aur
  (c) 60 s window ke andar hona — teeno lazmi hain.
- Disclosure **ek dafa** hai: window ke baad key mangne par board inkar karta hai.
- **Ehtiyat (honest limit):** key transfer ki confidentiality ka darja OPEN-6 ke
  bootstrap faisle par munhasir hai. Agar pehli pairing ke liye AP khula rakha
  jaye to transfer us window mein proximity + 60 s par hi munhasir hai — us
  waqt link-layer encryption nahi hoti. Is liye pairing ko kabhi bharay
  (public/shared) Wi-Fi par nahi, sirf user ke apne ghar/room mein karna hai;
  USER-GUIDE mein ye hidayat lazmi hai (Phase H docs). Ye residual risk hai —
  `THREAT-MODEL.md`, TM-W02.
- Key transfer ke baad tamam control traffic HMAC-signed hota hai (§1) — key
  ka dobara tabadla kabhi nahi hota.

**HELLO frame ka exact layout** (nonce format/length, key field, password field,
unpaired flag) WP3 ka kaam hai (§1 ke 9 items) — OPEN-7.

## 4. App side storage — DPAPI (c)

- App device key ko **Windows DPAPI, CurrentUser scope** se encrypt kar ke store
  karti hai — file `%AppData%\RSH` ke andar (rename §4: AppData folder RSH hai).
  Exact file naam / format Phase C/E implementation mein, is doc mein nahi.
- **DPAPI kya deta hai:** key disk par plain nahi rehti; doosra Windows user ya
  disk nikaal kar parhne wala key nahi khol sakta.
- **DPAPI kya nahi deta:**
  - Usi Windows user ke tor par chalne wala koi bhi process (malware samet)
    DPAPI decrypt kar sakta hai — DPAPI user-bound hai, app-bound nahi.
    Is liye key ki hifazat ka asal inhesar Windows user account + PC ki
    hifazat par hai (`THREAT-MODEL.md`, TM-P01–TM-P03).
  - DPAPI machine reinstall / user profile delete hone par key **kho jati hai**
    — is ka hal re-pairing hai (§6), backup nahi.
- **Mana hai:**
  - Key ko kabhi plain text file, log, crash dump, telemetry, settings export,
    ya UI mein dikhana mana hai (telemetry default OFF, D31; crash-dump redact).
  - Key ka **koi export / backup / cloud sync nahi** — key sirf is PC par rehti hai.
  - Key ko kabhi repo, git, ya kisi image mein nahi dalna.
- **In-memory:** key sirf zaroorat ke waqt memory mein rakho; app band hone par
  clear karo. Exact zeroisation Phase C/E mein implement hoga — design rule
  yahan darj hai, code nahi.

## 5. FACTORY_RESET vs KEY_RESET — farq (d)

|  | FACTORY_RESET (cmd 0x08) | KEY_RESET (naya command) |
|---|---|---|
| **Kya wipe hota hai** | Sirf pin map / config (factory defaults = v3.3 pin map) | Sirf device key (aur us se jura pairing state) |
| **Key ko kya hota hai** | **Kuch nahi — key rehti hai.** Is ke baad bhi purani key se HMAC valid hai; dobara pairing ki zaroorat nahi | **Key wipe + nayi key generate.** Purani key foran invalid; dobara pairing lazmi (§3) |
| **Config ko kya hota hai** | Wipe (factory defaults) | **Kuch nahi — config rehti hai** |
| **Auth** | HMAC-signed hona lazmi hai (purani key se) — bina key ke FACTORY_RESET nahi ho sakta | **Physical BOOT-hold lazmi hai** (neeche). Purani key ki zaroorat nahi — yehi is ka maqsad hai (key kho jaye to rasta) |
| **Kab istemal hota hai** | Ghalat pin map / config se wapas aana | Key kho jaye, app reinstall, board kisi doosre PC ko dena, ya key compromise ka shak |

**KEY_RESET ka tareeqa (design rule):**
1. User board ko power-off kar ke **BOOT (GPIO0) hold kar ke** power-on karta hai —
   physical maujoodgi ka saboot. Sirf software se KEY_RESET kabhi nahi hota.
2. Board "key-reset-armed" state mein aata hai (is state ki pehchan / LED
   indication WP3/Phase D mein — OPEN-8).
3. Is armed state mein (aur sirf is mein) app KEY_RESET command bhejti hai.
   Armed state ke bagair KEY_RESET hamesha reject hota hai.
4. Board purani key mita kar nayi key generate karta hai, unpaired mark hota hai,
   aur nayi 60 s pairing window kholta hai (§3 se pairing dobara).
5. Armed state waqt par khatam hoti hai (window jaisi muddat — WP3 confirm kare,
   OPEN-8); waqt guzar jaye to dobara BOOT-hold karna hoga.

**Takrao / OPEN — BOOT-hold ka do istemal:** purane firmware plan mein BOOT-hold
(≥ 5 s, power-up) safe mode (factory pin map + AP) ke liye tha. Ab BOOT-hold
KEY_RESET ke liye bhi hai. Dono states ko board kaise farq karega (hold ki
muddat? sequence? alag command?) — plan mein define nahi. **OPEN-9:** WP3 /
Phase D is ka faisla karega; jab tak band na ho, KEY_RESET implement nahi hoga.

**Command number:** KEY_RESET ka cmd byte plan mein assign nahi — WP3 assign
karega (FACTORY_RESET = 0x08 barqarar). OPEN-10.

## 6. Key kho jaye / app reinstall / board replace — re-pairing flow (e)

| Surat-e-haal | Kya hota hai |
|---|---|
| **App reinstall / PC reinstall / DPAPI key kho jaye** | App ke paas key nahi. Board abhi bhi purani key par paired hai aur nayi key nahi dega. **Hal:** §5 KEY_RESET (BOOT-hold) → board nayi key banata hai → §3 pairing dobara → purani key invalid. Sirf app dobara install karne se pairing wapas nahi aati. |
| **Board replace (naya board)** | Naye board ki apni nayi key hoti hai (§2). App purani key se naye board se baat nahi kar sakti. **Hal:** naye board se §3 pairing (pehla boot, 60 s window). Purane board ki key naye board par kabhi transfer nahi hoti. Profile v2 mein `boardMac` field (D21) se app pehchan sakti hai ke board badla hai — aur calibration dobara karni hogi (warning ke saath). |
| **Board dobara flash (USB-serial, poora erase)** | NVS erase hone par key bhi jati hai → agle boot par nayi key (§2 step 3) → §3 pairing dobara. |
| **Firmware OTA update (v3.3 → v4.0 ya v4.x → v4.y)** | OTA key NVS ko **nahi** chheerti — key aur pairing barqarar rehti hai. (Confirm Phase D mein — OPEN-11.) |
| **PC badal jaye (naya PC, purana board)** | Naye PC ki app ke paas key nahi → KEY_RESET + dobara pairing (§5). Key kabhi purane PC se naye PC mein copy nahi hoti. |
| **User board kisi aur ko de de** | Dene se pehle KEY_RESET karo — warna naya malik purani key ke bagair board use nahi kar sakta aur purana malik (agar us ke paas key ho) abhi bhi board ko control kar sakta hai. Ye USER-GUIDE mein likhna hai (Phase H). |

**Rule:** re-pairing mein hamesha **nayi key** banti hai; purani key dobara kabhi
valid nahi hoti; koi "purani key recover karo" ka rasta nahi hai.

## 7. Key rotation rule — kab hoti hai, kab nahi (f)

**Key rotation (device key badalna) sirf in mauqon par hoti hai:**
- KEY_RESET (§5) — user ki marzi se, BOOT-hold ke saath.
- Re-pairing after key loss (§6) — jo asal mein KEY_RESET hi hai.
- Board replace / poora reflash — naya board / nayi NVS = nayi key (§2, §6).

**Key rotation kabhi nahi hoti:**
- Routine reboot, OTA update, ya FACTORY_RESET par nahi.
- Waqt guzarne par (expiry) nahi — key ki koi muddat nahi hai.
- Har session / har HELLO par nahi — session badalne par sirf **nonce aur
  counter** badalte hain (neeche), key wahi rehti hai.

**Nonce / counter — ye key rotation nahi hai (D3, D30):**
- Counter **per-session** hai — naya nonce = counter 0 se shuru.
- Nonce **sirf tab** rotate hota hai jab board reboot ho ya PC naya session
  maange (HELLO mein session flag); routine 1 Hz HELO purana nonce wapas deta hai.
- Har control frame mein counter monotonic barhta hai; retry mein reqId wahi,
  counter naya; board (reqId, counter) pair se duplicate pehchanta hai (D3).
  Is se replay rokta hai — key badle bagair.
- Session khatam hone par purana nonce/counter dobara istemal nahi hota.

## 8. Wi-Fi / OTA password provisioning + forced change (g)

- **Random per-device:** Wi-Fi/OTA password board pehli pairing par generate
  karta hai (§3 step 4). **MAC se derive kabhi nahi** (MAC public hai, R-H10).
- **Ek dafa dikhana:** App password ko pehli pairing par **ek dafa QR / label
  ke tor par dikhati hai** — user use save / print kar le. Is ke baad app
  password ko plain kabhi nahi dikhati.
- **Forced change (NIST IR 8259A):** First setup par forced change — matlab
  user ko pehli pairing ke baad apna password set karne ka mauqa / taqaza
  hoga; factory wala generated password hamesha ke liye nahi rehta. Exact UX
  (app mein change flow, board par apply ka tareeqa) Phase D/E mein define
  hoga (OPEN-12).
- **Koi shared default nahi:** har board ka password mukhtalif hai; koi
  "admin/admin" jaisa default nahi.
- **OPEN-13 (WP3):** "Wi-Fi/OTA password" — kya ye **ek** password hai jo AP join
  aur WebOTA auth dono ke liye hai, ya **do alag** passwords (AP password alag,
  OTA password alag)? Plan ki wording se dono parhne mumkin hain. Faisla WP3
  ka hai; is doc mein dono ko ek hi credential mana **nahi** gaya.
- **OPEN-14 (WP3):** Password ki length / character set / generation ka tareeqa
  (recommendation, faisla nahi: kam az kam 20 characters, random — WP3 band kare).
- **Storage:** Board password ko encrypted NVS mein rakhta hai (§2 jaisi hifazat).
  App password ko plain file mein nahi rakhti — OS Wi-Fi profile is ka ghar hai;
  exact handling Phase E mein (OPEN-15).
- **Password bhool jaye to:** WebOTA / AP password bhoolne par hal KEY_RESET
  jaisa physical rasta hai ya FACTORY_RESET — dono mein se kaun sa password ko
  reset karta hai, ye WP3 decide karega (OPEN-16). Note: FACTORY_RESET key ko
  nahi wipe karta (§5) — password ke liye usi rule ko assume **mat** karo.

## 9. Secure Boot + image signing + anti-downgrade (h)

- **Secure Boot v2 = RSA-3072.** Sirf public key digest eFuse mein burn hota hai.
  Sirf us key se signed bootloader + app image boot hogi.
- **eFuse burning irreversible hai.** Ghalat key ya ghalat procedure board ko
  hamesha ke liye brick kar sakta hai. **Rule: pehle sacrificial board par test
  karo** — production / user ke board par pehli dafa kabhi nahi.
- **App-level image signature: Ed25519 / ECDSA P-256** (dono mein se ek —
  WP3/Phase D chunega, OPEN-17). Board image apply karne se **pehle verify**
  karta hai; verify fail ho to image reject, purani image chalti rehti hai.
- **Signed OTA Must hai** (MoSCoW, §7): bina signed OTA ke koi bhi binary flash
  kar sakta hai — is liye WebOTA par auth (F1, Phase D) + signature verify
  dono lazmi hain; ek doosre ka badal nahi.
- **Signing key offline rakho, kabhi repo / git / image mein nahi.** Signing
  sirf offline machine / release process se (Phase H release checklist).
  App signing (OV certificate, D23) is se alag hai — us ka certificate
  procurement Phase A mein shuru (WP8).
- **Anti-downgrade:** purani version wali image reject hogi. Version scheme:
  firmware v4.0.0 (D27); sirf barabar ya nayi version accept hoti hai.
  Anti-downgrade counter / eFuse version field ka exact mechanism WP3 /
  Phase D mein define hoga (OPEN-18).
- **Key compromise:** Secure Boot ya image-signing key compromise hone par kya
  hota hai (revocation? naye key se dobara sign? purane boards ka kya hoga?) —
  plan mein define nahi. **OPEN-19 (Phase H):** release process mein compromise
  response likhna hoga; is doc mein guess nahi kiya gaya.
- **Flash encryption:** §2 (OPEN-3) dekho — Secure Boot ke saath flash
  encryption enable karna hai ya nahi, Phase D ka faisla hai.

## 10. OPEN items — WP3 ya baad ke phases mein decide hone hain (i)

Ye list is doc ka hissa hai; koi bhi item guess kar ke implement **mat** karo.

| # | Item | Decide karega |
|---|---|---|
| OPEN-1 | Purani `PROTOCOL-V2.md` (key = OTA password ka hash) superseded hai — WP3 us file ko is design ke mutabiq update kare; jab tak update na ho, us file ko authority mat mano | WP3, Phase A |
| OPEN-2 | Control-channel key ki length (recommendation: 32 byte; faisla WP3 ka) | WP3, Phase A |
| OPEN-3 | Encrypted NVS ka exact mechanism + flash encryption enable karna hai ya nahi | WP3 / Phase D |
| OPEN-4 | Pehle boot par RNG tayyari + key-generation failure ka behaviour (fail closed — tareeqa WP3 ka) | WP3 / Phase D |
| OPEN-5 | Pairing window ka exact start trigger (AP-up se 60 s — confirm) + window khatam hone ki nishani | WP3, Phase A |
| OPEN-6 | **Pehli pairing ka bootstrap:** PC pehli dafa AP se kaise jure jab AP password abhi app ko maloom nahi (circular dependency) | WP3, Phase A — pairing implement karne se pehle lazmi |
| OPEN-7 | HELLO frame ka byte-level layout: nonce format/length, key field, password field, unpaired flag | WP3, Phase A (§1 ke 9 items) |
| OPEN-8 | KEY_RESET armed state ki pehchan / indication + armed window ki muddat | WP3 / Phase D |
| OPEN-9 | BOOT-hold ka do istemal (safe mode vs KEY_RESET) — board farq kaise karega | WP3 / Phase D |
| OPEN-10 | KEY_RESET ka cmd byte number (FACTORY_RESET = 0x08 barqarar) | WP3, Phase A |
| OPEN-11 | OTA update key NVS ko nahi chheerti — Phase D mein confirm karna hai | Phase D |
| OPEN-12 | Forced password change ka exact UX / flow (app + board) | Phase D / E |
| OPEN-13 | Wi-Fi password aur OTA password: ek credential ya do alag | WP3, Phase A |
| OPEN-14 | Wi-Fi/OTA password ki length / character set / generation | WP3, Phase A |
| OPEN-15 | App side AP password storage ka exact tareeqa (OS Wi-Fi profile — confirm) | Phase E |
| OPEN-16 | AP/OTA password bhoolne par reset ka rasta (KEY_RESET ya FACTORY_RESET — kaun sa) | WP3, Phase A |
| OPEN-17 | App-level signature algorithm: Ed25519 ya ECDSA P-256 — ek chunna hai | WP3 / Phase D |
| OPEN-18 | Anti-downgrade ka exact mechanism (version field / eFuse counter) | WP3 / Phase D |
| OPEN-19 | Signing / Secure Boot key compromise hone par response (revocation, re-sign) | Phase H |
| OPEN-20 | Wired transport (USB-serial) par pairing / key transfer hota hai ya sirf Wi-Fi par — spec TBD (D26) | Phase G |

## 11. Traceability — Plan v2.3 §2 ka har point kahan cover hai

| Plan §2 point | Is doc mein |
|---|---|
| Koi factory nahi, user ghar par flash karta hai | §0.1, §2 |
| Key pehle boot par board khud random generate, encrypted NVS | §2 |
| Pehle HELLO mein app ko deta hai, ek dafa, 60 s window | §3 |
| App key DPAPI se store karti hai | §4 |
| FACTORY_RESET key nahi wipe karta; KEY_RESET alag, BOOT-hold | §5 |
| App key kho jaye to dobara pairing, nayi key, purani invalid | §6 |
| Wi-Fi/OTA password random per-device, pehli pairing par, QR/label ek dafa | §3, §8 |
| MAC se derive kabhi nahi | §0.6, §8 |
| Secure defaults NIST 8259A: unique AP password, forced change, koi shared default nahi | §0.7, §8 |
| Secure Boot v2 RSA-3072 (eFuse digest) | §9 |
| App-level signature Ed25519/ECDSA P-256, verify before apply | §9 |
| eFuse irreversible, sacrificial board par test | §9 |
| Signing key offline, kabhi repo/git/image mein nahi | §9 |
| Anti-downgrade: purani version reject | §9 |
| HMAC over nonce‖counter‖cmd‖payload, per-device key | §1, §7 |
| Counter per-session; naya nonce = counter 0 | §7 (D3, D30) |
| Nonce rotation sirf reboot / naya session; routine HELO purana nonce | §7 (D30) |
| DTLS evaluate + defer — local AP + HMAC design hai | §0.8 |
| STRIDE threat model | `THREAT-MODEL.md` (sathi document) |

---

*Doc type: design only. Is mein likha koi bhi rule code nahi hai — implementation
WP3 (protocol), Phase C/E (app), Phase D (firmware), Phase H (release) mein hogi,
is doc aur OPEN list ke mutabiq.*
