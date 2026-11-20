# RSH — Threat Model (STRIDE, Rust series, v2.0.6)

**Status:** DESIGN — this document models threats and assigns
mitigations to phases; it implements nothing. Code comes in the
phases named per threat.
**Date:** 2026-11-20
**Series:** App Rust **v2.0.6** · Firmware **C++ v4.0.3** · Protocol
**v2.2 wire FROZEN**.
**Authority:** `ARCHITECTURE.md` §2.5 (security model) ·
`SECURITY-INPUT.md` §1 (threat model TM-R01–R08), §8 (carried
items), §9 (phase map), §10 (residual risks) · `PROTOCOL-V2.2.md`
(wire authority).
**Companion document:** `PROTOCOL-V2.2.md` (the wire rules this
model stands on — HELLO layout, reqId dedup, SET_PASSWORD,
WebOTA gating).
**Deleted (2026-11-20, v2.0.6):** `KEY-LIFECYCLE-RUST.md` — the
device-key lifecycle is dead with pairing. Every reference to it
in this document has been removed or rewritten; do not revive it.
**Standards (unchanged):** OWASP ISVS v1.0 (2025) + NIST IR 8259A
+ ETSI EN 303 645.

> **Headline:** pairing is abolished (user order 2026-11-20).
> There is no device key, no HMAC, no pairing window, no
> `Authenticating` state. **The WPA2 Wi-Fi password is now the
> ONLY security boundary.** Anyone who knows it can control the
> board and trigger WebOTA; anyone on the open AP before a
> password is set can send control frames and set a password
> first. The largest remaining risk is the open-AP window before
> the user sets a password (TM-W02) — it is bounded by the
> first-connect prompt, not by cryptography. No Critical/High
> threat is left open without a plan; every open item in §5
> names the phase that fixes it.

---

## 0. Method and scope

- **Method:** STRIDE — Spoofing, Tampering, Repudiation, Information
  Disclosure, Denial of Service, Elevation of Privilege. For each threat:
  attack, impact, mitigation, **the phase that fixes it**, and residual risk.
- **Threat IDs:** C# IDs (`TM-Wxx`, `TM-Oxx`, `TM-Pxx`) are **kept, never
  renumbered**. New Rust/Tauri threats are `TM-R01–R08` (SECURITY-INPUT §1).
- **Assumptions (this model stands on these):**
  1. No factory / cloud / internet service — only the local Wi-Fi SoftAP
     and wired USB-serial between board and PC.
  2. An attacker can be in Wi-Fi range of the board; they cannot attack
     from the internet (the board is never on the internet).
  3. The user's PC and Windows account are under the user's control
     (if the PC is compromised, the keyring goes with it — TM-P01).
  4. No software claim is made against a physical attacker holding the
     board — BOOT-hold exists precisely because physical presence is
     authority, and chip-level attacks (decapping, glitching) are out of
     scope.
  5. DTLS was evaluated and **deferred** — local AP + WPA2 password is
     the design, not an oversight. Transport protection rests on AP
     isolation + the Wi-Fi password. There is no HMAC and no replay
     window in protocol v2.2 — a captured frame can be replayed within
     the reqId-dedup scope (TM-W03); this is an accepted LAN-only risk.
  6. The WebView2 frontend is treated as **hostile** (Tauri security
     model, first principle) — see the new trust boundary in §1.
- **Assets:** (1) Wi-Fi/OTA password — the only credential, (2) pin map /
  config, (3) firmware image, (4) PC profiles + calibration, (5) Secure
  Boot / image-signing keys (offline), (6) Tauri updater signing key
  (offline, separate from (5)). **There is no device key anymore.**

---

## 1. Trust boundaries and exposed surfaces

```
[ Internet ] --(no route)--> [ Board SoftAP ] <-- Wi-Fi --> [ PC app ]
                                  |  UDP 4210/4211/4212 (HELO/stream/control)
                                  |  WebOTA (HTTP, auth day-1, disabled until
                                  |    a password is set)
                                  |  HELLO (no secret, reserved fields zero)
                                  |  USB-serial (RSHS, 921600 baud)
[ Physical attacker ] -- BOOT-hold (10 s) --> [ Board (eFuse, flash, NVS) ]
[ PC user / malware ] -- keyring ----> [ Windows Credential Manager : wifi pw ]
[ npm / crates.io ] --(build-time)--> [ Frontend bundle / Rust binary ]

NEW boundary (Rust/Tauri — did not exist in C#):
[ Web frontend (bundled, local) ] -- invoke()/IPC --> [ Rust core (full OS access) ]
        ^ WebView2 (Chromium, OS-provided)              ^ keyring, UDP, serial, FFI, FS
[ npm deps (build-time) ]            [ Tauri capabilities = allow-list ]
```

**Exposed surfaces:**

| # | Surface | Section |
|---|---|---|
| A | Wi-Fi SoftAP + UDP (+ wired USB-serial) | §2 (TM-W01–W08) |
| B | WebOTA (HTTP) | §3 (TM-O01–O04) |
| C | PC app (credentials, settings, profiles) | §3 (TM-P01–P05) |
| D | **NEW — Tauri frontend ↔ Rust core (IPC, WebView2, supply chain, FFI)** | §4 (TM-R01–R08) |

---

## 2. Surface A — Wi-Fi SoftAP + UDP (+ wired serial)

Board SoftAP (default IP 192.168.4.1, mDNS `rsh.local` first,
fallback IP; SSID `RSH-XXXX`, last 4 hex of the board Wi-Fi MAC,
uppercase, fixed per board) + UDP ports 4210 (HELO), 4211
(stream), 4212 (control), and wired USB-serial (`RSHS`,
921600 baud). Stream packets (200 Hz) and **plain, unsigned**
control frames run on both transports with the same
session rules (PROTOCOL-V2.2.md §6; C# OPEN-20 carried).

### TM-W01 — Spoofing: fake board (evil twin / rogue AP)

- **Attack:** an attacker opens an AP with the board's SSID; the app joins
  it, and the fake board sends bogus GET_CAPS / stream data to the PC —
  or the app talks to the fake board instead of the real one.
- **Impact:** Medium — there is no key to leak anymore (HELLO carries no
  secret), but the app can be fed wrong caps / wrong input, and the user
  can be tricked into setting a password on the wrong board.
- **Rust status: REWRITTEN for v2.2.** No pairing window to abuse and no
  key to hand over. Board binding by `boardMac` (D11) is kept: the HELLO
  `board_mac` must match the SSID suffix of the board the app joined, and
  a mismatch is a `BoardMismatch` error, not a warning. The app joins by
  the scanned `RSH-XXXX` SSID, never by a stale profile named `RSH`.
- **Fix phase:** Engine/Protocol (binding gate) + UI/Tauri (SSID picker).
- **Residual risk:** Medium. No cryptographic fix exists for first
  identity before the board is known. Mitigation: connect only at
  home/in your own room (first-run guidance, mandatory).

### TM-W02 — Information Disclosure: open-AP window before a password is set

- **Attack:** the board's AP is **open** from first boot (or after a
  BOOT-hold KEY_RESET) until the user sets a Wi-Fi password. Anyone in
  Wi-Fi range in that window can join the AP, send control frames
  (TM-W08), and — worst case — **set a password first** (SET_PASSWORD
  with `curLen 0` is allowed while none is set), locking the owner out
  until a physical BOOT-hold KEY_RESET.
- **Impact:** High while the window is open — full board control for
  anyone in range.
- **Rust status: REWRITTEN for v2.2 — this replaces the old TM-W02
  (key sniffed in the first HELLO), which is DEAD: HELLO carries no
  secret in v2.2 (its key/password fields are reserved/zero,
  PROTOCOL-V2.2.md §2.2).** The exposure is now the open AP itself,
  not a disclosure. Mitigations: the wizard prompts for a password
  **immediately after first connect** (Settings → Connection card);
  WebOTA is **disabled until a password is set** (TM-O01), so the
  worst an attacker in the window can do is control, not reflash;
  SET_PASSWORD requires the current password once one is set
  (TM-W08), so the lock-out only works before the first password.
- **Fix phase:** Firmware/OTA (WebOTA gating) + UI/Tauri (first-connect
  password prompt; the prompt must not be skippable without an
  explicit "remind me later" that re-prompts on every connect).
- **Residual risk:** Medium-High while no password is set; **Low once
  set**. This is the largest open point in the model — the window is
  bounded by user behaviour, not by cryptography, and the app must
  say so plainly.

### TM-W03 — Tampering / Replay: control frame altered or replayed

- **Attack:** an attacker captures a UDP control frame and replays it
  (e.g. FACTORY_RESET twice) or alters the payload (e.g. a wrong pin map
  in SET_CFG).
- **Impact:** High — wrong config, factory reset, board misuse.
- **Rust status: REWRITTEN for v2.2 — HMAC is GONE.** There is no
  per-device key, no nonce, no counter, no tag. Control frames are
  plain (`"RSHC" | cmd | reqId | len | payload | CRC16`,
  PROTOCOL-V2.2.md §3.1). Tamper is caught by CRC16-CCITT (an altered
  packet is dropped — the same protection the stream always had).
  Replay is caught **only by reqId dedup**: a u16 reqId, monotonic per
  session, last-32 cache — a duplicate is answered from the cache and
  not executed again. A captured frame **can** be replayed after its
  reqId leaves the 32-entry window. This is an **accepted LAN-only
  risk behind the WPA2 password**, not an oversight; reopening it is
  an Architecture-phase decision, never a silent change. The old
  `BAD_TAG` (1) error is deleted; `REPLAY` (2) is kept as a wire value
  but never sent. Golden vectors (PROTOCOL-V2.2.md §3.3) must match
  byte-identically (Rust app + C++ firmware), or the gate fails.
- **Fix phase:** Engine/Protocol (reqId dedup, vectors) + Firmware/OTA
  (board-side dedup).
- **Residual risk:** Medium (control channel). The WPA2 password is the
  only thing standing between an AP joiner and control.

### TM-W04 — Tampering: stream packets (input data) altered

- **Attack:** an attacker alters stream packets (wrong steering/pedal data
  into the game) or drops packets to raise loss.
- **Impact:** Medium — wrong in-game input; no config or key leak.
- **Rust status: UNCHANGED design residual (Medium-Low).** CRC16-CCITT on
  every stream packet — an altered packet is dropped; status packets
  carry seqGap + txFail counters so loss is visible. **Stream has no
  HMAC — a design decision, not an oversight, and now the same is true
  of control (TM-W03).** Reopening it is an Architecture-phase
  decision, never a silent change.
- **Fix phase:** Engine/Protocol (vectors) + Firmware/OTA. HMAC-on-stream
  stays out of scope unless Architecture reopens it explicitly.
- **Residual risk:** Medium-Low. The attacker can only DoS / inject wrong
  input (a dropped packet = loss); they cannot take control — except
  through the control channel (TM-W03), which is the real exposure.

### TM-W05 — Denial of Service: restart / flood / deauth

- **Attack:** (a) WebOTA restart endpoint abused (see TM-O01); (b) UDP
  flood exhausts the board's loop/heap; (c) Wi-Fi deauth knocks the PC off
  the AP; (d) malformed/oversized frames crash the parser; (e) HELO
  flood / session churn.
- **Impact:** High (wheel dead mid-race) — Medium (deauth is local only).
- **Rust status: REWRITTEN for v2.2.** Mitigations carried: (a) auth on every
  endpoint (TM-O01, day-1 in Rust firmware) **and WebOTA disabled until
  a password is set**; (b) heap-threshold restart rule + full-loop timing;
  (c) no software fix for deauth — the wired transport is the answer;
  (d) strict length/bounds checks (axes ≤ 8, buttons ≤ 128, encoders ≤ 4;
  over → reject) + parser host tests + fuzz. **HELO acceptance rules
  (SEC-D-08, carried):** a second IP cannot overwrite `pcIp` in a live
  session; flap throttle 5 s; HELLO-reply rate limit 1 per 500 ms; stream
  is emitted only after a plain `GET_CAPS` OK in this session
  (SEC-E-14, redefined — no HMAC anymore) — the app must send GET_CAPS
  first on every (re)connect. **New-session gating (SEC-R2-04)** is the
  open part — §5: `sessionFlag=1` no longer rotates a nonce (none
  exists), so the DoS surface is HELO flood / session churn, not
  credential rotation.
- **Fix phase:** Firmware/OTA (rate limits, SEC-R2-04 fix) + Engine/Protocol
  (parser, fuzz) + Release (rig DoS test gate).
- **Residual risk:** Medium. Deauth / RF jamming has no software fix;
  wired transport is the answer.

### TM-W06 — Spoofing: a second device joins the AP and controls the board

- **Attack:** another device (a household member, a guest) joins the board's
  AP and sends control commands.
- **Impact:** High — joining the AP **is** enough now. There is no HMAC
  key to stop a second device: anyone on the AP can send control frames
  (TM-W08) and the stream is visible.
- **Rust status: REWRITTEN for v2.2.** The old mitigation ("HMAC key
  required for any control frame") is dead with pairing. The only gate
  is the WPA2 password: a second device needs it to join at all. AP
  client limit / single-PC policy is a Firmware-phase decision (carried
  open from C# — not decided here, not hidden).
- **Fix phase:** Firmware/OTA.
- **Residual risk:** Low while a strong password is set (control needs
  the password); **High on an open AP** (TM-W02 window).

### TM-W07 — Information Disclosure: flash / NVS dump extracts the password

- **Attack:** an attacker with the board in hand dumps flash (USB-serial /
  flash reader) and reads the Wi-Fi password from NVS.
- **Impact:** High if successful — full control (the password is the only
  boundary).
- **Rust status: REWRITTEN for v2.2 — there is no device key to dump
  anymore (OPEN-3 carried).** The board password is **obfuscated in NVS
  with a fresh random NVS key generated once at first 4.0.3 boot**
  (never the deleted device key); no encryption is claimed. Secure Boot
  v2 RSA-3072 (eFuse digest) means only signed images boot, so an
  attacker cannot flash their own image to extract the password — once
  Secure Boot is enabled (board-side execution is still pending, §5).
  In Track B (esp-rs) encrypted NVS is a day-1 requirement, not a later
  item.
- **Fix phase:** Firmware/OTA + Release (eFuse burn + dump verification on
  a sacrificial board first, then production; closure criteria rewritten
  for the Rust series from the C# procedure).
- **Residual risk:** Medium until OPEN-3 closes; any software claim
  against a physical attacker is limited by definition (§0, assumption 4).

### TM-W08 — Elevation of Privilege: unauthenticated command changes config

- **Attack:** an attacker sends SET_CFG / FACTORY_RESET / REBOOT without
  any credential and changes the board's config or reboots it.
- **Impact:** High.
- **Rust status: REWRITTEN for v2.2 — this is now TRUE, by design.**
  Every control command is unauthenticated: without a Wi-Fi password
  set, anyone on the AP can send SET_CFG / FACTORY_RESET / REBOOT and
  the board executes them. FACTORY_RESET (0x08) clears the password +
  config and reboots to an open AP; REBOOT (0x09) needs no credential.
  KEY_RESET has **no wire opcode anymore** (0x0B deleted, replies
  `UNSUPPORTED`) — password-clear is physical-only: hold BOOT ≥ 10 s
  and the firmware deletes the NVS password itself. The mitigations
  are: (1) the WPA2 password, once set, is the only boundary
  (TM-W02/TM-W06); (2) SET_PASSWORD requires the **current** password
  once one is set (PROTOCOL-V2.2.md §3.6), so an AP joiner cannot lock
  the owner out after the first password — but only after it. There is
  **no unauthenticated restart distinction anymore** — REBOOT is
  unauthenticated by design, and the only "public" commands are all of
  them. This is an **accepted risk**, recorded here, not an oversight.
- **Fix phase:** Engine/Protocol (command table — the table now records
  "unauthenticated, AP-gated") + Firmware/OTA.
- **Residual risk:** High on an open AP (TM-W02 window); Medium-Low once
  a strong password is set.

---

## 3. Surface B — WebOTA (HTTP) · Surface C — PC app

WebOTA stays with **auth from day 1** in Rust firmware — in C# this
was a Phase D fix; in Rust it is a starting requirement, never a
"fix it later" item. WebOTA stays on HTTP (local AP, no TLS — a per-device
certificate is not feasible; DTLS/TLS defer stands). **New in v2.0.6:
WebOTA is disabled until a Wi-Fi password is set** — a board with no
password (fresh, or after KEY_RESET) answers every WebOTA request with
`401`, and an empty password never authenticates. Consequence, stated
in the first-run guidance: set a Wi-Fi password in Settings first, and
use WebOTA only on your own home AP.

### TM-O01 — Auth Bypass (F1): firmware upload / restart without a password — FORMERLY THE BIGGEST

- **Attack:** (C# audit finding F1) some WebOTA endpoints had no auth; an
  attacker on the AP uploaded arbitrary firmware or restarted the board
  repeatedly without authentication.
- **Impact:** Critical in C# before the fix.
- **Rust status: FIXED BY DESIGN, carried as a day-1 requirement, and
  EXTENDED in v2.0.6.** **Auth on every handler** — root, `/update`,
  restart, and not-found included; the auth check is the first line of
  every route. No endpoint exists without auth. **Plus: WebOTA is
  disabled until a password is set** — with no password there is no
  auth credential at all, so nothing is served. Upload additionally
  requires the image signature (TM-O02) and anti-downgrade; auth and
  signature are both mandatory, neither substitutes for the other.
- **Fix phase:** Firmware/OTA (day-1, not deferred). Gate: endpoint-by-endpoint
  auth test in the rig, including the no-password `401` case.
- **Residual risk:** Low after the fix.

### TM-O02 — Tampering: firmware image swapped in transit

- **Attack:** an attacker swaps the image during OTA upload, or uploads an
  incomplete / corrupt image.
- **Impact:** High — bricked or compromised firmware.
- **Rust status: UNCHANGED.** Image signature **Ed25519, verified before
  apply**; size/magic check (magic 0xE9, size cap = app partition);
  anti-downgrade (only equal or newer version accepted; version scheme
  redefined for the Rust series, not compared to C# v4.0.x); app verifies
  before upload (`ed25519-dalek` `verify_strict`, baked public key — the
  upload button is not even enabled until verification passes); board
  verifies again after upload. Stalled-upload DoS guard (C# Round 2
  firmware fix) carried. Brick recovery doc + test carried.
- **Fix phase:** Firmware/OTA + UI/Tauri (upload UI) + Release (signing
  workflow, fail-closed).
- **Residual risk:** Low.

### TM-O03 — Information Disclosure / Brute Force: OTA password guessed

- **Attack:** an attacker guesses the WebOTA password (brute force /
  dictionary). The AP password and the OTA password are **the same
  credential** (OPEN-13/14 — CLOSED in v2.0.6, see §5): one guess opens
  both, by design.
- **Impact:** High if a guess succeeds (= TM-O01).
- **Rust status: REWRITTEN for v2.2.** The password is **user-set,
  8–63 printable ASCII** (Settings → Wi-Fi password; validated on the
  board, PROTOCOL-V2.2.md §3.6) — not a generated 24-char password
  anymore. Strength now depends on the user: the Settings UI must show
  a strength hint and refuse weak/short input. **Lockout: 5 wrong
  attempts → 60 s lockout, for everyone** (C# values carried). The
  lockout is itself a repeatable lockout-DoS — **an accepted tradeoff,
  not a fix** (SEC-R2-05, carried); during lockout even the correct
  password is rejected, by design; the first-run guidance says so.
- **Fix phase:** Firmware/OTA (lockout + constant-time compare — SEC-R2-07
  is a day-1 fix in Rust, not "document only" as in C#) + UI/Tauri
  (password strength hint).
- **Residual risk:** Medium-Low (SEC-R2-05 accepted tradeoff) — higher
  than before if the user picks a weak password; the strength hint is
  the only mitigation.

### TM-O04 — Repudiation / CSRF: browser triggers WebOTA

- **Attack:** a browser on the user's PC (joined to the board's AP) is
  tricked by a malicious page into requesting a WebOTA endpoint (restart
  or upload trigger).
- **Impact:** Medium.
- **Rust status: UNCHANGED.** Auth on every endpoint (TM-O01) — the
  browser has no password, so the request fails. Plus a **per-boot random
  CSRF token** (`X-RSH-CSRF` header), obtainable only from the
  authenticated page; mandatory on state-changing requests
  (upload/restart/password change) with constant-time comparison.
- **Fix phase:** Firmware/OTA.
- **Residual risk:** Low.

### TM-P01 — Information Disclosure: password stolen from the credential store (malware / other user)

- **Attack:** malware running as the same Windows user opens the keyring
  entry and extracts the Wi-Fi password; or the password leaks in plain
  text in a log / crash dump / settings export.
- **Impact:** High — the password is the only boundary; it gives full
  board control.
- **Rust status: SAME LIMIT, NEW (SMALLER) SECRET.** `keyring` (Windows
  Credential Manager, DPAPI-backed) is user-bound, exactly like C# DPAPI
  — the **same** limit: malware as the same user can open it. What is
  stored is now only the **Wi-Fi password** (`board:<MAC>:wifi`); there
  is no device key to steal. Password never in a log / crash dump /
  telemetry / export / UI; `tracing` log-redaction tests in CI; crash
  dumps opt-in, default OFF, redacted; telemetry default OFF. No export
  / backup / cloud sync. **Honest limit:** no app-level design removes
  this; PC hygiene is the user's responsibility and the first-run
  guidance says so. If compromise is suspected, BOOT-hold KEY_RESET
  (10 s) clears the password and ends the exposure.
- **Fix phase:** Engine/Protocol (`keyring` store, `zeroize` handling) +
  Release (docs, redaction gate).
- **Residual risk:** Medium. PC compromise = password compromise.

### TM-P02 — Tampering: profile / settings file altered

- **Attack:** an attacker or a bad import alters a profile (wrong
  calibration, wrong pin map, wrong output mapping) — or the app itself
  migrates a profile wrongly.
- **Impact:** Medium — wrong output, data loss.
- **Rust status: UNCHANGED rules, re-implemented.** Profile v2 + schema
  version + migration; 3-generation backup (rotate on every save; a
  corrupt file never overwrites a good backup); import/export async +
  size caps (profile import 1 MB cap, settings 64 KB cap — TM-R02) +
  version check + full validation; backup zip never contains password
  fields.
- **Fix phase:** Engine/Protocol (profiles) + UI/Tauri (import UX).
- **Residual risk:** Low.

### TM-P03 — Information Disclosure: credentials in installer / logs / crash dump

- **Attack:** the installer, logs, or a crash dump leaks the Wi-Fi
  password; or an old AppData folder keeps credentials after a migration.
- **Impact:** Medium-High.
- **Rust status: UNCHANGED rules, stricter in Rust.** No secret in logs —
  not plain, not hashed, not the first characters, not even the length
  (length is a leak too — do not log it). Secret-bearing types mask or
  omit secrets in `Debug`. Log files under `%AppData%\RSH\logs`,
  current-user-only permissions, rotation + size cap. Redaction is a CI
  test (log-output scan: 0 hits for password/key/token/secret/credential/
  auth), not a manual check. App is **unsigned** (OV cert deferred) —
  SmartScreen warning expected; the release notes say so; users are
  never told to disable SmartScreen. Firmware signing is unaffected.
- **Fix phase:** Foundation (redaction CI test) + Engine/Protocol +
  Release (installer, docs).
- **Residual risk:** Low.

### TM-P04 — Elevation of Privilege: malicious profile / firmware file executes code

- **Attack:** the user is given a malicious profile or firmware `.bin`;
  the app opens or flashes it and code runs.
- **Impact:** High.
- **Rust status: UNCHANGED rules, re-implemented.** Import: size cap +
  version check + full validation. Firmware upload: size/magic check +
  Ed25519 signature verify **before** upload (TM-O02) — the board never
  applies a bad image. vJoy DLL is **never bundled** — resolved
  only from registry / Program Files, SHA-256 pinned; if missing, show a
  download link. SBOM (CycloneDX, Rust + frontend) + THIRD-PARTY-LICENSES.md
  with every release.
- **Fix phase:** Engine/Protocol (validation) + Firmware/OTA + Release (SBOM).
- **Residual risk:** Low-Medium (social-engineering a user into opening a
  bad file is always possible — first-run guidance warning).

### TM-P05 — Denial of Service / Repudiation: single instance and autostart

- **Attack:** two app instances fight over vJoy / UDP ports; or autostart
  opens the app in a wrong state.
- **Impact:** Low-Medium.
- **Rust status: CARRIED INTO TM-R08** (same threat, Tauri mechanism).
  See TM-R08 below.
- **Fix phase:** UI/Tauri (TM-R08).
- **Residual risk:** Low.

---

## 4. Surface D — NEW Rust/Tauri threats (TM-R01–R08)

These eight threats did not exist in the C# (WPF) series. Each names its
severity, mitigations, and fixing phase.

### TM-R01 — XSS → command abuse (High)

- **Attack:** the WebView is treated as hostile (Tauri security model,
  first principle). Any XSS — or any compromised frontend dependency —
  can call every Tauri command the window's capabilities allow: firmware
  upload, config writes, password change.
- **Mitigations:**
  (a) Strict CSP — `app.security.csp`: `default-src 'self'; script-src
  'self'; style-src 'self'` (`'unsafe-inline'` only in style-src for
  Tailwind, never in script-src); `object-src 'none'; base-uri 'self';
  frame-ancestors 'none'; connect-src 'self' ipc: http://ipc.localhost` —
  no remote origin, no `unsafe-eval`.
  (b) Capabilities per window, least-privilege —
  `src-tauri/capabilities/*.json`, every command/plugin permission
  explicit, no `*`, no default-allow, no `remote.urls` grant (the
  frontend is bundled local assets only).
  (c) **Every command re-validates on the Rust side** — frontend validation
  is UX, not security: type/length/range/enum checks on every argument,
  and a state check on every privileged command (upload, password
  change, config write: connected? password set?).
  (d) **No secret ever reaches the frontend** — the Wi-Fi password is
  never returned to the frontend; the frontend shows only masked/status
  information ("password set ✓" — not even the last 2 characters of a
  password). **The pairing-password carve-out is DEAD with pairing:**
  there is no one-time pairing password anymore, no single-use consume
  flag, no `PairingStatus` disclosure — nothing is disclosed to the
  frontend, full stop. (The old carve-out text lived in
  `KEY-LIFECYCLE-RUST.md` §3, deleted with that document in v2.0.6.)
- **Fix phase:** Foundation (CSP + capability template) + UI/Tauri (per-command
  validation with each command).
- **Residual risk:** Medium (mitigated, not eliminated — a compromised
  frontend dependency is always possible; see TM-R03).

### TM-R02 — IPC input validation gap (High)

- **Attack:** Tauri commands deserialize JSON — a wrong/malicious payload
  (a huge string, a negative axis, an out-of-range pin) reaches Rust code.
- **Mitigations:** `serde` bounds + explicit validation in every command
  (the C# A9/A10 size-cap and version-check rules, re-implemented in Rust:
  profile import 1 MB cap, settings 64 KB cap, firmware image size cap,
  pin-map validator); oversized payloads rejected; fail closed.
- **Fix phase:** Engine/Protocol + UI/Tauri.
- **Residual risk:** Low after validation (each command carries its own
  validation; the gate is per-command tests, including oversized and
  out-of-range payloads).

### TM-R03 — Frontend supply chain (Medium)

- **Attack:** npm dependencies (Vite/React/Tailwind/shadcn) are build-time
  code — a compromised dependency lands in the bundled JS and opens the
  TM-R01 path.
- **Mitigations:** `package-lock.json` committed; `npm audit` CI gate
  (fail on High/Critical); dependencies pinned, avoiding `^` ranges where
  security-sensitive; lockfile review on every dependency bump; no `eval`
  and no `innerHTML` with untrusted data in the frontend; CSP (TM-R01 (a))
  as the second defence.
- **Fix phase:** Foundation (CI) + UI/Tauri.
- **Residual risk:** Medium-Low (audit gates reduce it; a zero-day
  compromised dependency has no complete fix).

### TM-R04 — Rust supply chain: build.rs / proc-macro / typosquat (Medium)

- **Attack:** crates.io has no mandatory code review; a `build.rs` runs
  arbitrary code on every `cargo build` (full filesystem/network access,
  CI secrets in reach); proc-macro crates can read `env::vars()` at
  macro-expansion time; typosquatted crate names trick a developer into
  adding the wrong dependency.
- **Mitigations:** `cargo audit --deny warnings` + `cargo deny check`
  (advisories + licenses + bans + sources) as a **CI gate on every PR and
  every release**; `Cargo.lock` committed; `deny.toml` with sources =
  crates.io only, git denied by default, a banned-crates list (including
  typosquat-prone names); `cargo tree` review + `cargo-geiger` (unsafe
  count) report when adding a dependency; crates with `build.rs` need an
  explicit justification on an allow-list; CI secrets (signing keys) only
  in the release job, never in PR builds.
- **Fix phase:** Foundation (CI first — the C# WP6 principle: without CI,
  no phase's "tests pass" counts).
- **Residual risk:** Medium-Low (gates reduce it; a zero-day compromised
  crate has no complete fix).

### TM-R05 — WebView2 version / patching (Low-Medium)

- **Attack:** Tauri uses the OS-provided WebView2 (Chromium). Benefit: OS
  patches arrive with the OS. Cost: an old WebView2 on the user's PC is
  an old Chromium with known vulnerabilities.
- **Mitigations:** WebView2 bootstrapper in the installer (download or
  embedded — UI-phase decision, keeping offline install in mind); a
  minimum WebView2 version check at startup, with a warning + update link
  if too old; a fixed/embedded runtime **only** if offline install makes
  it mandatory — and then its patching becomes our responsibility,
  recorded in the release notes.
- **Fix phase:** UI/Tauri + Release.
- **Residual risk:** Low (mitigated by the version check; the user must
  still apply the update).

### TM-R06 — Tauri plugin over-grant (Medium)

- **Attack:** every plugin (fs, dialog, opener, updater, single-instance,
  autostart) asks for permissions; a wrong capability hands the frontend
  filesystem / shell / process access.
- **Mitigations:** only the plugins actually needed; `fs` scope limited to
  `%APPDATA%\RSH` (and the profiles dir where required); **no `shell`
  plugin** (no `shell:allow-execute`); `opener` for https links only;
  every capability file gets a security review at the release gate, with a
  written reason for each permission.
- **Fix phase:** UI/Tauri.
- **Residual risk:** Low (least-privilege capabilities + release-gate
  review).

### TM-R07 — Unsafe FFI boundary (Medium)

- **Attack:** vJoy (C DLL) and the HIDMaestro native API run through
  `unsafe` FFI — Rust's memory safety ends at this boundary. A bad pointer,
  a wrong struct layout, or an unchecked return value from the C side can
  corrupt memory or crash the app.
- **Mitigations:** `unsafe` confined to one small FFI module, with a
  `// SAFETY:` comment on every `unsafe` block (stating what invariant the
  caller guarantees); `unsafe` never spread into business logic; no secret
  ever crosses the FFI boundary (FFI functions receive only axis/button/
  report data); every value coming back from FFI (report size, axis range,
  button count) is validated on the Rust side — a C DLL return is never
  blindly trusted; buttons > 32 rejected, never clamped/aliased (C# rule
  carried); vJoy DLL SHA-256 pinned, never bundled; HIDMaestro
  interop path decided by the G1 spike (its native API from Rust is still
  **[UNVERIFIED]** — if no native API exists, vJoy-only v1.0.0 or a
  sidecar design, each with its own threat analysis).
- **Fix phase:** Engine/Protocol (FFI module) + Release gate (FFI audit).
- **Residual risk:** Medium-Low (contained by the module boundary and the
  SAFETY audit; the boundary itself cannot be made memory-safe).

### TM-R08 — Single-instance / autostart (Low)

- **Attack:** two app instances fight over vJoy / UDP ports (C# TM-P05,
  same threat, Tauri mechanism); autostart opens the app in a wrong state.
- **Mitigations:** `tauri-plugin-single-instance` — the second instance
  focuses the first window and never starts a new engine; version-aware
  (an older version is terminated — C# rule, fail-closed); autostart
  verified (registry/run key, Windows side).
- **Fix phase:** UI/Tauri.
- **Residual risk:** Low.

---

## 5. Open items — tracked, with fixing phases

**No Critical/High threat is left open without a plan.** Every item below
names its fixing phase. "Carried" means the C# status is unchanged and the
item is not hidden; "Decided" means this document records the decision and
its reason; "Dead" means the item died with pairing in v2.0.6 and must not
be reopened as a pairing item.

| ID | Severity | Status | Fixing phase |
|---|---|---|---|
| **SEC-R2-01** — no in-app firmware upload in classic mode (wizard-only) | Medium | **FIX in Rust by design.** Tauri has no "classic mode / wizard skip": firmware upload lives in the **main window** (Settings/Diagnostics or a dedicated Firmware page, via a `tauri::command`), with the verify-first rule (TM-O02) — the upload button is not enabled until the image is verified. The wizard-only upload design is not rebuilt. Gate: upload end-to-end from the main UI. | UI/Tauri |
| **SEC-R2-03** — unpaired board's AP stays open indefinitely after the 60 s window | Medium | **DEAD with pairing (v2.0.6).** There is no window and no "unpaired" state anymore — the AP is open until the user sets a password, by design (TM-W02). The successor item is the open-AP window: the first-connect password prompt + WebOTA disabled until a password is set. Do not revive SEC-R2-03 as a pairing item. | — (dead; successor TM-W02) |
| **SEC-R2-04** — unauthenticated HELO `sessionFlag=1` rotates the nonce and resets both control sessions (DoS) | Medium | **Open — fix defined, not yet implemented, REDEFINED for v2.2.** There is no nonce to rotate anymore, so the DoS surface is HELO flood / session churn. Rate-limit new-session requests (e.g. one new session per N seconds, or only when no session is active). Exact limit + rig DoS test in the Firmware phase. DoS only, never control. | Firmware/OTA (with Engine) |
| **SEC-R2-06** — no board-binding enforcement in the key store (evil twin inside the 60 s window can overwrite the stored key/password) | Medium | **DEAD with pairing (v2.0.6).** There is no stored key to overwrite. Board binding survives as the `BoardMismatch` gate (HELLO `board_mac` must match the joined SSID suffix — TM-W01, D11), enforced in the Engine/Protocol phase. | — (dead; binding kept as BoardMismatch) |
| **SEC-R2-07** — WebOTA Basic-auth compare not constant-time | Low | **FIX in Rust firmware, day-1** (in C# this was "document only" because of the lockout; in Rust the constant-time compare is mandatory from the start). | Firmware/OTA |
| **SEC-R2-05** — WebOTA lockout is itself a repeatable lockout-DoS | Low | **Accepted tradeoff, documented** (TM-O03). No fix; first-run guidance records it. | — (accepted) |
| **SEC-R2-02** — legacy plaintext Wi-Fi password wipe lazy/not guaranteed | Low | **DROPPED with reason** — fresh series, no legacy plaintext store exists; password fields are excluded from any C# settings import. Reopens only if a C# migration is ever built. | — (closed) |
| **SEC-H-04** — password-compromise response, board-side execution | — | **CARRIED to Release.** The C# doc is rewritten for the Rust series (new keypair/version scheme); board-side execution (password-clear drill: BOOT-hold 10 s → open AP → set new password) happens at the A7-style real-board run. The doc review is not re-done here. | Release |
| **OPEN-3** — flash encryption + encrypted NVS, board-side | — | **CARRIED to Firmware/OTA + Release.** Encrypted NVS is a day-1 requirement for Rust firmware (Track B); for Track A (C++ v4.0.3) the password is obfuscated in NVS with a fresh random NVS key (never the deleted device key) until the eFuse burn + dump verification runs on a sacrificial board first. Closure criteria rewritten for the Rust series. Until then TM-W07 residual Medium stands, and the release notes say so. | Firmware/OTA + Release |
| **OPEN-6** — pairing bootstrap | — | **DEAD with pairing (v2.0.6).** There is no bootstrap anymore: first connect is an open AP, then the user sets a password in Settings. The wired-primary / open-AP-fallback distinction is gone — USB is just a second transport, same connect-play path. | — (dead) |
| **OPEN-19** — password compromise response / revocation | — | **Carried as a document to be rewritten for the Rust series (SEC-H-04 above).** | Release |
| **OPEN-13/14** — one credential vs two (Wi-Fi vs OTA password); length/charset | — | **CLOSED (v2.0.6): one credential — the user-set Wi-Fi password, 8–63 printable ASCII, is the WebOTA password too** (TM-O03, PROTOCOL-V2.2.md §3.6). The old discrepancy (Architecture §2.5 said one credential, SECURITY-INPUT §5 listed it undecided) is resolved in favour of one credential; the choice must not change silently in code. | — (closed) |
| **OPEN-4** — RNG readiness / failure behaviour | — | Rule carried (fail closed); mechanism to be defined — now for the NVS obfuscation key generated at first 4.0.3 boot, not for a device key. | Firmware (Phase D) |
| **OPEN-5** — pairing-window start trigger | — | **DEAD with pairing (v2.0.6).** No window exists. | — (dead) |
| **OPEN-11** — OTA does not touch password NVS | — | Carried — confirm in Firmware phase (OTA must never clear or rotate the stored Wi-Fi password; the 4.0.3 first-boot secret wipe is a one-time migration, not an OTA behaviour). | Firmware (Phase D) |
| **OPEN-12** — forced password-change UX | — | Carried, reframed: the first-connect password prompt (TM-W02) — the wizard must prompt for a password right after first connect and re-prompt on every connect until one is set. | UI/Tauri + Firmware |
| **OPEN-18** — anti-downgrade mechanism (Rust version scheme) | — | Carried — version scheme redefined for the Rust series. | Firmware/Release |

### Decisions recorded in this document (v2.0.6)

1. **Pairing is abolished (user order 2026-11-20).** No device key, no
   HMAC, no pairing window, no `Authenticating` state, no wire
   KEY_RESET. The WPA2 Wi-Fi password is the only security boundary.
   (PROTOCOL-V2.2.md §0.)
2. **First connect is an open AP; the user sets a password in Settings
   right after.** WebOTA stays disabled until a password is set. The
   open-AP window is bounded by the first-connect prompt, not by
   cryptography — and the model says so (TM-W02).
3. **KEY_RESET is physical-only.** Hold BOOT ≥ 10 s; the firmware
   deletes the NVS password itself and reboots to an open AP. No wire
   opcode, no armed window, no app needed. (PROTOCOL-V2.2.md §5.)
4. **OPEN-13/14 closed: one credential.** The user-set Wi-Fi password
   is the WebOTA password too (TM-O03).

These decisions are frozen: a later phase may implement them, but may
not re-decide them without amending this document first. The two A5
decisions (SEC-R2-03 WPA2-after-window, OPEN-6 wired-primary bootstrap)
died with pairing and are removed, not carried.

---

## 6. Residual-risk table (risks that remain after v2.0.6 — stated, not hidden)

| Risk | Level | Why it remains |
|---|---|---|
| TM-W02 open-AP window (no password set yet) | Medium-High | Bounded by the first-connect prompt, not by cryptography; anyone in range can join and set a password first. |
| TM-W08 unauthenticated control (anyone on the AP) | High on open AP / Medium-Low once password set | No HMAC by design; the WPA2 password is the only gate. |
| TM-W03 control replay (reqId dedup only) | Medium | A captured frame can be replayed after its reqId leaves the 32-entry window; accepted LAN-only risk. |
| TM-W07 flash dump (OPEN-3) | Medium | eFuse burn + dump verification on a real board is still pending; password is obfuscated, not encrypted, in NVS. |
| TM-P01 PC malware takes the password (keyring is user-bound) | Medium | The OS keyring is user-bound, not app-bound — PC compromise = password compromise. No app design removes this. |
| TM-R01 XSS → command abuse | Medium (mitigated) | CSP + capabilities + Rust-side validation reduce it; a compromised frontend dependency is always possible. |
| TM-R04 supply chain (build.rs / typosquat) | Medium-Low | Audit gates reduce it; a zero-day compromised crate has no complete fix. |
| TM-W05 deauth / RF DoS | Medium | No software fix; the wired transport is the answer. |
| TM-W04 stream tampering | Medium-Low | No HMAC on the stream (design decision). |
| TM-O03 OTA brute force (lockout-DoS tradeoff; user-chosen password strength) | Medium-Low | SEC-R2-05 accepted tradeoff; strength now depends on the user + the Settings strength hint. |
| TM-R07 unsafe FFI boundary | Medium-Low | The boundary cannot be made memory-safe; it can only be contained and audited. |
| HIDMaestro Rust API unverified | Medium (phase gate) | Decision after verification — if no native API exists, vJoy-only v1.0.0 or a sidecar design. |
| TM-R05 old WebView2 on the user's PC | Low-Medium | Mitigated by the startup version check; the user must still apply the update. |

---

## 7. Traceability checklist — Architecture §2.5, rule by rule

Every rule in `ARCHITECTURE.md` §2.5 must trace to a line in
`PROTOCOL-V2.2.md` (P) or this document (TM). Result: **all 9 rules
trace. Nothing could not be traced.** The OPEN-13/14 discrepancy flag
from the v1.0.0 checklist is **closed** (rule 4).

| # | Architecture §2.5 rule | Traced to |
|---|---|---|
| 1 | Control channel is plain (no HMAC, no key): `"RSHC" | cmd | reqId | len | payload | CRC16`; duplicates caught by reqId only (last 32); no unauthenticated-restart distinction — REBOOT is unauthenticated by design, AP-gated. | P §3.1, §2.3; TM §2 TM-W03, TM-W08 |
| 2 | No device key exists. HELLO 92 B: `device_key[32]`, `wifi_password[24]`, `nonce[16]` are reserved/zero; only `safe_mode` flag bit is defined; reserved-nonzero is rejected. | P §2.2; TM §2 TM-W02 |
| 3 | Connect-play bootstrap: AP open until the user sets a password; on first connect the app prompts for a password; WebOTA disabled until a password is set. No link-layer encryption in the window — proximity + prompt; first-run guidance must direct connecting only at home/in your own room. | P §7, §8; TM §2 TM-W02, §5 (decisions 1–2) |
| 4 | Wi-Fi/OTA password: **one credential** for both; user-set, 8–63 printable ASCII; SET_PASSWORD carries the current password when one is set. | P §3.6; TM §3 TM-O03. **OPEN-13/14 CLOSED** (was a discrepancy flag in v1.0.0). |
| 5 | Signed firmware (Must): **Ed25519**, verify **before apply**, anti-downgrade (version < running → reject). App verifies before upload (`ed25519-dalek`, verify-only). Signing key outside the repo, CI secret; public key baked in firmware. | TM §3 TM-O02 |
| 6 | Web OTA: HTTP Basic Auth on **every** handler, lockout, CSRF; ArduinoOTA disabled; **disabled until a password is set**. | TM §3 TM-O01, TM-O03, TM-O04 |
| 7 | Open items carried: SEC-R2-01, SEC-R2-04, OPEN-3, OPEN-19. (SEC-R2-03, SEC-R2-06, OPEN-5, OPEN-6 dead with pairing; OPEN-13/14 closed.) | TM §5 table |
| 8 | HELO acceptance (SEC-D-08): a second IP cannot overwrite `pcIp` in a live session; flap throttle 5 s; HELLO-reply rate limit 1 per 500 ms; stream only after a plain `GET_CAPS` OK in this session (SEC-E-14, redefined) — the app must send GET_CAPS first on every (re)connect. | TM §2 TM-W05 (rules stated in full) |
| 9 | App signing: app **unsigned** (OV cert deferred — user decision); SmartScreen warning expected. Firmware signing unaffected. | TM §3 TM-P03 |

**Checklist result: 9 of 9 rules traced; 0 rules untraceable.** No
discrepancy flag remains.

---

## 8. Phase-wise mitigation map

| Phase | Threats |
|---|---|
| Foundation | TM-R03 (npm audit gate), TM-R04 (cargo audit/deny gates, lockfiles, deny.toml, secret-scan), TM-R01 (CSP + capability template), TM-P03 (log-redaction CI test) |
| Engine / Protocol | TM-W03, TM-W08, TM-W04 (vectors), TM-R02 (command validation), TM-R07 (FFI module), TM-P01/P03 (keyring store — Wi-Fi password only, zeroize), TM-P02 (profiles), TM-W01 (BoardMismatch gate) |
| Firmware / OTA | TM-O01 (auth day-1 + disabled-until-password), TM-O02, TM-O03, TM-O04, TM-W05 (rate limits), SEC-R2-04 (new-session gating), SEC-R2-07 (constant-time compare), OPEN-3 (encrypted NVS day-1 for Track B), TM-W02 (WebOTA gating) |
| UI / Tauri | TM-R01, TM-R02, TM-R05, TM-R06, TM-R08, SEC-R2-01 fix (upload in main window), OPEN-12 (first-connect password prompt), TM-O03 (password strength hint) |
| Release | TM-O02 (signing workflow, fail-closed), TM-P03/P04 (SBOM, installer, docs), SEC-H-04 + OPEN-19 (password-clear drill), OPEN-3 (eFuse burn + dump verification, sacrificial board first), TM-R05 (WebView2 bootstrapper decision), TM-R07 (FFI audit) |

---

*Doc type: design only. No mitigation in this document is code. If a phase
finds a mitigation unworkable, the mitigation is amended here first; it is
never changed silently in code. Open items are tracked in §5 and in the
residual-risk table in §6 — they are not hidden, and no Critical/High item
is open without a named fixing phase.*
