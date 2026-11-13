# RSH FW-3 safe mode + FW-4 config transaction — implementation note

**Phase D, Workstream 1.** Authority: `docs/PROTOCOL-V2.1.md` §3.5/§3.6
(SET_CFG chunk layout, config blob, error table), §4.4 (BOOT-hold bands),
Plan v2.3 D28 (confirm timer hamesha 30 s), D12/D13. Firmware plan:
`RSH-FIRMWARE-PLAN.md` FW-3/FW-4.

## Files

| File | Kya hai |
|---|---|
| `firmware/rsh/safemode.h` | FW-3 pure logic (Arduino-free, host-tested) |
| `firmware/rsh/config_transaction.h` | FW-4 pure logic (Arduino-free, host-tested) |
| `firmware/rsh/status_led.{h,cpp}` | `SafeMode` (1 Hz) + `KeyResetArmed` (4 Hz) LED states |
| `firmware/rsh/rsh.ino` | NVS glue + boot setup + per-loop tick (FW-3/FW-4 block) |
| `tests-host/test_main.cpp` §12–13 | Host tests (safe mode + config transaction) |

## FW-3 — safe mode + boot counter (`safemode.h`)

- **Runtime BOOT-hold (C1 fix)** — power-up par nahi. Loop har iteration
  GPIO0 (BOOT button, active-LOW) parhta hai (`rsh_boothold_update`):
  hold < 5 s → ignore; **5 s ≤ hold < 10 s → safe mode**; **hold ≥ 10 s →
  key-reset-armed**. Release hote hi detector reset. LED: safe mode slow
  blink 1 Hz, armed fast blink 4 Hz (§4.4). Armed window 120 s — waqt
  guzar jaye to board normal mode mein reboot hota hai. Safe mode se
  reboot ke baghair bahar nahi nikla jata.
- **Boot fail = power-on ke 15 s mein AP up na ho.** `rsh_boot_tick` har
  loop mein chalta hai (net down ho tab bhi); natija ek dafa darj hota
  hai (der se AP up hona fail wapas nahi karta). **3 lagatar fail →
  last-known-good; LKG bhi 3 dafa fail → factory.** Kamyab boot dono
  counter zero karta hai. LKG har kamyab boot par refresh hoti hai
  (current config → LKG slot).
- **Safe mode mein config apply nahi hoti** — `rsh_cfg_set_safe_mode`
  stage + commit dono ko `BUSY` (7) se rokta hai. Normal LED state safe
  mode ki LED ko overwrite nahi karti (`.ino` mein guard).
- NVS (`rsh-boot` namespace): `total` / `fails` / `lkgFails` (u32).

## FW-4 — config transaction (`config_transaction.h`)

- **Stage → commit → confirm (30 s) → rollback.** Chunk ≤ 1024 B, total
  ≤ 2048 B. Chunks **kisi bhi order mein** (§3.5) — bitmap se track hote
  hain; dobara bheja gaya chunk idempotent hai. Naya session (offset 0,
  **alag total**) purana staged buffer discard karta hai. Har chunk ka
  CRC16 verify hota hai; poore blob ka **CRC32 trailer** (§3.6) COMMIT
  par verify hota hai.
- **Confirm timer hamesha 30 s (D28)** — reboot ho ya live apply.
  COMMIT reply mein `confirmDeadlineSec = 30`. Dobara COMMIT (retry)
  deadline dobara shuru **nahi** karta (idempotent).
- **Rollback:** confirm na ho 30 s mein → `rsh_cfg_tick` purani config
  wapas lata hai (previous-copy rollback). Der se CONFIRM bhi pehle
  rollback karta hai, phir `UNSUPPORTED` deta hai. `REVERT` idempotent
  hai (kuch pending na ho to no-op OK).
- **Reboot confirm nahi karta (D28 faisla):** board ke paas wall clock
  nahi hoti (uptime reboot par zero), is liye reboot ke doran confirm
  window mein maujood commit **rollback** hota hai (`rsh_cfg_boot_recover`
  + NVS `pend` flag). Baaqi waqt ka andaza nahi lagaya jata — fail-safe.
  Caller invariant: `pend` flag aur `previous` copy hamesha ek saath
  likhe jayen.
- **Validation (§3.6 err codes):** magic/schema → err 3 (`UNSUPPORTED`);
  size mismatch → err 5 (`TOO_BIG`); CRC32 → err 4 (`BAD_CRC`); pin
  conflict → err 1, out-of-range (axes > 8, encoders > 4, pin > 39) →
  err 2 (`PIN_CONFLICT`); buttons 65–128 → err 3 (`UNSUPPORTED`, N1).
  Bad blob COMMIT par reject hota hai — live config chhooi nahi jati.
- **Factory blob** = v3.3 pin map (steering 34, accel 35, brake 36,
  buttons 25/26/27/33) — host test `config.h` ke `PIN_*` se match karta hai.
- NVS (`rsh-cfg` namespace): `cur` / `lkg` / `prv` slots (1024 B chunks:
  `<pfx>Len` + `<pfx>0` + `<pfx>1`), `pend` u8. Rollback/timeout par
  restored current persist hoti hai aur `pend` clear hota hai.

## Control-channel wiring (FW-7/8 follow-up — DONE 2026-11-13)

Live control channel (port 4212, HMAC) ab FW-3/FW-4 commands ke liye
wired hai — `net.cpp` ka `net_ctrl_handler` dispatcher (`ctrl.h`) se
`rsh_fw34_handler` (`ctrl_handlers.h`, pure/host-testable) ko delegate
karta hai:

- **GET_CFG (0x02):** offset|wantLen (4 B) → totalLen|offset|data.
  wantLen > 1024 → `TOO_BIG`; data 1019 B par clamp hoti hai
  (dispatcher body had 1023 B: 1 status + 4 header + 1019 data).
  Safe mode mein bhi chalta hai (read block nahi hoti).
- **SET_CFG (0x03):** offset|total|chunkLen|chunkCrc|data — chunks kisi
  bhi order mein, idempotent resend. Reply hamesha stagedBytes|total.
- **COMMIT (0x04):** len 0. OK → body `[30]` (confirmDeadlineSec) +
  NVS persist (`cur` + `prv` + `pend=1`). Dobara COMMIT wahi deadline
  deta hai (idempotent, cache se). Validation fail → body
  `[errCount, errCode × n]` (pehli 8 codes, status = pehla err).
- **CONFIRM (0x05):** len 0. OK → persist `pend=0`. Der se (deadline
  ke baad) → `UNSUPPORTED` (pehle rollback ho chuka hota hai).
- **REVERT (0x06):** rollback + persist `pend=0` (sirf jab kuch pending
  tha — no-op par NVS write nahi).
- **KEY_RESET (0x0B):** len 0. Armed nahi → `UNSUPPORTED`. Armed ho to
  handler sirf pending flag lagata hai; asal rotation + reboot **reply
  bhejne KE BAAD** hoti hai (`net_poll_ctrl` → `keystore_key_reset()`:
  nayi key + naya password NVS mein, unpaired → `ESP.restart()`).
  Wajah: rotation reply se pehle hoti to reply ka HMAC tag nayi key se
  banta aur PC (purani key wala) verify na kar pata. Rotate fail ho to
  reboot NAHI (fail closed — purani key rehti hai; armed window expiry
  par normal reboot hota hai). REBOOT (0x09) wala deferred pattern
  (SEC-D-01).
- **Armed-state ki ek source of truth:** `keystore_key_reset_armed()`
  ab `rsh_fw34_key_reset_armed()` (rsh.ino, `fw34_state.h`) parhta hai —
  is se HELLO flags bit3 aur GET_CAPS capsFlags bit3 ka maujooda code
  baghair tabdeeli ke sahi ho gaya. HELLO bit2 aur GET_CAPS bit1
  (safeMode) bhi `rsh_fw34_safe_mode_active()` se wired hain.
- **FACTORY_RESET (0x08)** scope se bahar hai — `UNSUPPORTED` rehta hai.
- Host tests §21 (`tests-host/test_main.cpp`): asal `rsh_ctrl_dispatch`
  se — full transaction (out-of-order chunks → COMMIT `[30]` + persist
  spy → CONFIRM → GET_CFG roundtrip), timeout rollback + der se CONFIRM,
  REVERT, bad-blob COMMIT (`BAD_CRC` + `[1,4]`), safe-mode BUSY
  (stage/commit `BUSY`, GET_CFG OK), KEY_RESET unarmed/armed/payload,
  bad tag → `BAD_TAG` (handler nahi chalta), duplicate COMMIT → cached
  reply, malformed payloads, GET_CFG clamp (2044 B blob → 1019 B data).

## Scope boundary (honest)

- **Live control channel (port 4212, HMAC)** ab wired hai (upar dekho) —
  FW-3/FW-4 commands dispatcher se execute hote hain. Baqi reh gaya:
  blob → pins application (neeche).
- **Blob → pins application** (config se inputs chalana) is workstream
  mein nahi hai — inputs abhi compiled-in `PIN_*` se chalte hain.
  Transaction layer config ko store/version/rollback karti hai; blob ke
  pins ko inputs par apply karna FW-5/FW-11 ka follow-up hai. Is liye
  "config apply" yahan matlab: txn `current` + NVS persistence.
- **Real board par test nahi hua** (user faisla: real board tests sirf
  final production release par). Verification: host tests (mock) + rig.

## Contract discrepancy (PROTOCOL-V2.1.md §3.6)

Blob header ke baare mein doc mein tazad hai: "Header (8 B)" likha hai,
lekin fields ki list (`'RSHG' | schema | moduleCount | cfgVersion u16 |
flags u8 | reserved u8`) 10 B banti hai. Is implementation mein **8 B**
size ko tarjeeh di gayi hai (4+1+1+2 = 8, `flags`/`reserved` bytes nahi
hain) — `rsh_cfg_blob_build` / `rsh_cfg_blob_validate` is layout par
hain. Agar coordinator 10 B header chahta hai to §3.6 update + yahan
`RSH_CFG_HEADER_SIZE` badalna hoga.

## Parallel-work note (coordinator ke liye)

Isi repo mein parallel kaam mein `firmware/rsh/safe_mode.h` aur
`firmware/rsh/config_txn.h` bhi bane hain (alag design). Farq:

- `safe_mode.h`: BOOT-hold **2 s** (contract §4.4 ke 5 s/10 s bands ke
  khilaf), koi key-reset-armed state / 120 s window nahi, koi 15 s AP
  deadline tick nahi (glue par chhora hua). Is workstream ki `safemode.h`
  contract ke mutabiq hai — aur doosre agent ke host tests (§11) bhi
  **is** file ke khilaf likhe gaye hain (5 s/10 s/15 s/120 s).
- `config_txn.h`: chunks **contiguous-only** (contract §3.5 "kisi bhi
  order mein" ke khilaf), blob integrity **CRC16** (contract CRC32
  trailer ke khilaf), `stage_chunk` signature wire SET_CFG layout se
  match nahi karti (blob_crc/ver params — wire mein nahi hain), koi
  live-apply/previous-copy nahi, koi validation/err codes nahi, koi
  REVERT ya boot-recover nahi. Is workstream ki `config_transaction.h`
  contract ke mutabiq hai.

**Faisla darkar hai:** dono mein se ek engine rakhna hai. Tajweez: is
workstream ki files (`safemode.h`, `config_transaction.h`) rakho aur
`safe_mode.h` / `config_txn.h` hatao — kyun ke ye contract-faithful
hain aur host tests (donon agents ke) in ke khilaf pass hote hain.
Doosre agent ke `config_txn.h` ke tests (§12) hataane se pehle unhein
is engine ke API par port karna hoga. **Is workstream ne doosre agent
ki files ko chhedda nahi hai** (sirf un ke ek test mein 1-line state
fix — `using_lkg` reset, test_main.cpp:1505 — kyun ke un ka test
`rsh_boot_counter_t` ko dobara istemal karte waqt stale flag chhor
raha tha; API ka rawaiya sahi tha).

**RESOLUTION — Phase E WS5 (2026-11-13): faisla ho gaya aur laagu ho
gaya.** `config_txn.h` hata diya gaya; waahid live engine
`config_transaction.h` hai. §12 ke tests live API par port hain
(`tests-host/test_main.cpp` §12) — do semantic farq wahan darj hain
(confirm boundary: live `>=` 30 s rollback; COMMITTED mein stage =
BUSY). `safe_mode.h` pehle hi repo mein nahi hai (sirf `safemode.h`
hai). `config_txn.h` ka koi include ab baaqi nahi.
