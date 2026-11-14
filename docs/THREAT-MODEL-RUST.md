# RSH — Threat Model (STRIDE, Rust series, v1.0.0)

**Status:** DESIGN — no implementation yet. This document models threats and
assigns mitigations to phases; it implements nothing. Code comes in the
phases named per threat.
**Date:** 2026-11-14
**Series:** App Rust **v1.0.0** (fresh start, Tauri 2, no port of C# code) ·
Firmware **C++ v4.0.1** (Track A ships v1.0.0; Track B esp-rs is a spike
only) · Protocol **v2.1 wire FROZEN**.
**Authority:** `RSH-RUST-ARCHITECTURE.md` §2.5 (security model) ·
`SECURITY-INPUT.md` §1 (threat model TM-R01–R08), §8 (carried items), §9
(phase map), §10 (residual risks) · `RSH-RUST-PLAN.md` §6 (security summary),
§9 (open decisions).
**C# baseline (read-only reference):** `reference/docs/THREAT-MODEL.md`
(TM-W01–W08, TM-O01–O04, TM-P01–P05), `KEY-LIFECYCLE.md`, `SECURITY.md`,
`PROTOCOL-V2.1.md` §4–§5, `SIGNED-OTA.md` (C# app v2.0.4 / firmware v4.0.1,
re-scan CONDITIONAL PASS, no Critical/High).
**Companion document:** `KEY-LIFECYCLE-RUST.md` (key lifecycle design — the
mitigations here point at its sections).
**Standards (unchanged):** OWASP ISVS v1.0 (2025) + NIST IR 8259A + ETSI
EN 303 645.

> **Headline:** no Critical/High threat is left open without a plan. Every
> open item in §5 names the phase that fixes it. The largest remaining risk
> is the same as in C#: the first-pairing bootstrap (TM-W02), now narrowed
> by the wired-primary decision (§4, OPEN-6) to the open-AP fallback path
> only.

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
  5. DTLS was evaluated and **deferred** — local AP + HMAC v2.1 is the
     design, not an oversight. Transport protection rests on AP isolation
     + HMAC + the pairing window.
  6. The WebView2 frontend is treated as **hostile** (Tauri security
     model, first principle) — see the new trust boundary in §1.
- **Assets:** (1) device key, (2) Wi-Fi/OTA password, (3) pin map /
  config, (4) firmware image, (5) PC profiles + calibration, (6) Secure
  Boot / image-signing keys (offline), (7) Tauri updater signing key
  (offline, separate from (6)).

---

## 1. Trust boundaries and exposed surfaces

```
[ Internet ] --(no route)--> [ Board SoftAP ] <-- Wi-Fi --> [ PC app ]
                                  |  UDP 4210/4211/4212 (HELO/stream/control)
                                  |  WebOTA (HTTP, auth day-1)
                                  |  HELLO / pairing (60 s window)
                                  |  USB-serial (RSHS, 921600 baud)
[ Physical attacker ] -- BOOT-hold --> [ Board (eFuse, flash, NVS) ]
[ PC user / malware ] -- keyring ----> [ Windows Credential Manager : key, wifi pw ]
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
| C | PC app (credentials, settings, keys, profiles) | §3 (TM-P01–P05) |
| D | **NEW — Tauri frontend ↔ Rust core (IPC, WebView2, supply chain, FFI)** | §4 (TM-R01–R08) |

---

## 2. Surface A — Wi-Fi SoftAP + UDP (+ wired serial)

Board SoftAP (default IP 192.168.4.1, configurable, D33; mDNS `rsh.local`
first, fallback IP) + UDP ports 4210 (HELO), 4211 (stream), 4212 (control),
and wired USB-serial (`RSHS`, 921600 baud). Stream packets (200 Hz) and
HMAC-signed control frames run on both transports with the same
session/auth rules (protocol v2.1 §10; C# OPEN-20 carried).

### TM-W01 — Spoofing: fake board (evil twin / rogue AP)

- **Attack:** an attacker opens an AP with the board's SSID; the app joins
  it, sends HELLO, and hands credentials to the fake board — or the fake
  board sends bogus GET_CAPS / stream data to the PC.
- **Impact:** High — a leaked key hands control of the real board to the
  attacker.
- **Rust status: UNCHANGED.** Pairing only in the 60 s window + first HELLO
  (KEY-LIFECYCLE-RUST §3); per-device random AP password (NIST 8259A);
  board binding by `boardMac` (D21, Profile v2) with the SEC-R2-06
  **gate** (not a warning) in the Engine/Protocol phase (§5).
- **Fix phase:** Engine/Protocol (SEC-R2-06 gate) + UI/Tauri (pairing UX).
- **Residual risk:** Medium. No cryptographic fix exists for first
  identity before the board is known. Mitigation: pair only at home/in
  your own room (USER-GUIDE, mandatory).

### TM-W02 — Information Disclosure: key sniffed in the first HELLO

- **Attack:** an attacker sniffs Wi-Fi traffic during the pairing window
  and captures the device key + Wi-Fi/OTA password from the first HELLO.
- **Impact:** High — the key signs every control command.
- **Rust status: UNCHANGED in design, NARROWED in exposure.** Disclosure is
  once-only, 60 s, local AP (KEY-LIFECYCLE-RUST §3). The OPEN-6 decision
  (§4) makes **wired USB-serial pairing the primary bootstrap**: on the
  primary path the key never crosses the air at all. The residual below
  now applies to the **open-AP fallback path only**.
- **Fix phase:** Firmware/OTA (OPEN-6 implementation) + UI/Tauri (pairing UX
  prefers the wired path; USER-GUIDE warning on the fallback).
- **Residual risk:** Medium-High on the fallback path; Low on the wired
  path. This remains the largest open point in the model — which is why
  OPEN-6 had to be decided before pairing is implemented.

### TM-W03 — Tampering / Replay: control frame altered or replayed

- **Attack:** an attacker captures a UDP control frame and replays it
  (e.g. FACTORY_RESET twice) or alters the payload (e.g. a wrong pin map
  in SET_CFG).
- **Impact:** High — wrong config, factory reset, board misuse.
- **Rust status: UNCHANGED (protocol v2.1 frozen).** HMAC over
  `nonce ‖ counter ‖ cmd ‖ payload`, per-device 32 B key, tag = first
  16 bytes (D3); wrong tag → `BAD_TAG` (1); old counter → `REPLAY` (2);
  retry keeps reqId, new counter; board recognises duplicates from the
  (reqId, counter) 32-entry cache. Rust re-implements this and re-runs the
  C# control-frame negative vectors (T2) — vectors must match
  byte-identically (Rust app + C++ firmware), or the gate fails.
- **Fix phase:** Engine/Protocol (HMAC, replay cache) + Firmware/OTA
  (board-side verify).
- **Residual risk:** Low (control channel). Remaining risk is session
  handling detail, verified by the golden vectors (Phase A1 gate).

### TM-W04 — Tampering: stream packets (input data) altered

- **Attack:** an attacker alters stream packets (wrong steering/pedal data
  into the game) or drops packets to raise loss.
- **Impact:** Medium — wrong in-game input; no config or key leak.
- **Rust status: UNCHANGED design residual (Medium-Low).** CRC16-CCITT on
  every stream packet (D2) — an altered packet is dropped; status packets
  carry seqGap + txFail counters (D5) so loss is visible. **Stream has no
  HMAC — a design decision, not an oversight.** Reopening it is an
  Architecture-phase decision, never a silent change.
- **Fix phase:** Engine/Protocol (vectors) + Firmware/OTA. HMAC-on-stream
  stays out of scope unless Architecture reopens it explicitly.
- **Residual risk:** Medium-Low. The attacker can only DoS / inject wrong
  input (a dropped packet = loss); they cannot take control.

### TM-W05 — Denial of Service: restart / flood / deauth

- **Attack:** (a) WebOTA restart endpoint abused (see TM-O01); (b) UDP
  flood exhausts the board's loop/heap; (c) Wi-Fi deauth knocks the PC off
  the AP; (d) malformed/oversized frames crash the parser.
- **Impact:** High (wheel dead mid-race) — Medium (deauth is local only).
- **Rust status: UNCHANGED.** Mitigations carried: (a) auth on every
  endpoint (TM-O01, day-1 in Rust firmware); (b) heap-threshold restart
  rule (D17) + full-loop timing (D15); (c) no software fix for deauth —
  the wired transport is the answer; (d) strict length/bounds checks
  (D6: axes ≤ 8, buttons ≤ 128, encoders ≤ 4; over → reject) + parser
  host tests + fuzz. **HELO acceptance rules (SEC-D-08, carried):** a
  second IP cannot overwrite `pcIp` in a live session; flap throttle 5 s;
  HELLO-reply rate limit 1 per 500 ms; stream is emitted only after at
  least one HMAC-verified control command was ACCEPTed in this session
  (SEC-E-14) — the app must send a signed GET_CAPS first on every
  (re)connect. **New-session gating (SEC-R2-04)** is the open part — §5.
- **Fix phase:** Firmware/OTA (rate limits, SEC-R2-04 fix) + Engine/Protocol
  (parser, fuzz) + Release (rig DoS test gate).
- **Residual risk:** Medium. Deauth / RF jamming has no software fix;
  wired transport is the answer.

### TM-W06 — Spoofing: a second device joins the AP and controls the board

- **Attack:** another device (a household member, a guest) joins the board's
  AP and sends control commands.
- **Impact:** Medium — joining the AP alone is not enough (HMAC key is
  needed), but the stream is visible and DoS is possible.
- **Rust status: UNCHANGED.** HMAC key required for any control frame
  (TM-W03); per-device random AP password; AP client limit / single-PC
  policy is a Firmware-phase decision (carried open from C# — not decided
  here, not hidden).
- **Fix phase:** Firmware/OTA.
- **Residual risk:** Low (control), Medium (sniffing/DoS — see TM-W02/W05).

### TM-W07 — Information Disclosure: flash / NVS dump extracts the key

- **Attack:** an attacker with the board in hand dumps flash (USB-serial /
  flash reader) and reads the key from NVS.
- **Impact:** High if successful — full control.
- **Rust status: UNCHANGED — carried (OPEN-3).** In Track A (C++ v4.0.1)
  the key is in **plaintext NVS** and the password is XOR-obfuscated only;
  no encryption is claimed. Secure Boot v2 RSA-3072 (eFuse digest) means
  only signed images boot, so an attacker cannot flash their own image to
  extract the key — once Secure Boot is enabled (board-side execution is
  still pending, §5). In Track B (esp-rs) encrypted NVS is a day-1
  requirement, not a later item (KEY-LIFECYCLE-RUST §11).
- **Fix phase:** Firmware/OTA + Release (eFuse burn + dump verification on
  a sacrificial board first, then production; closure criteria rewritten
  for the Rust series from the C# procedure).
- **Residual risk:** Medium until OPEN-3 closes; any software claim
  against a physical attacker is limited by definition (§0, assumption 4).

### TM-W08 — Elevation of Privilege: unauthenticated command changes config

- **Attack:** an attacker sends SET_CFG / FACTORY_RESET / REBOOT without a
  key and changes the board's config or reboots it.
- **Impact:** High.
- **Rust status: UNCHANGED.** Every control command must be HMAC-signed;
  without a valid tag the board executes nothing. FACTORY_RESET (0x08)
  needs a valid HMAC too (and does not wipe the key);
  KEY_RESET (0x0B) needs the physical BOOT-hold-armed state only. There
  is **no unauthenticated restart** (`RSTX` removed) — only HMAC-signed
  REBOOT. No command is "public" except HELLO/GET_CAPS — and GET_CAPS
  never carries the key.
- **Fix phase:** Engine/Protocol (command auth table) + Firmware/OTA.
- **Residual risk:** Low, provided HMAC is enforced on every command
  (the auth table is a Phase C/D checklist item, not an assumption).

---

## 3. Surface B — WebOTA (HTTP) · Surface C — PC app

WebOTA stays (D24) with **auth from day 1** in Rust firmware — in C# this
was a Phase D fix; in Rust it is a starting requirement, never a
"fix it later" item. WebOTA stays on HTTP (local AP, no TLS — a per-device
certificate is not feasible; DTLS/TLS defer stands). Consequence, stated
in the USER-GUIDE: use WebOTA only on your own home AP.

### TM-O01 — Auth Bypass (F1): firmware upload / restart without a password — FORMERLY THE BIGGEST

- **Attack:** (C# audit finding F1) some WebOTA endpoints had no auth; an
  attacker on the AP uploaded arbitrary firmware or restarted the board
  repeatedly without authentication.
- **Impact:** Critical in C# before the fix.
- **Rust status: FIXED BY DESIGN, carried as a day-1 requirement.**
  **Auth on every handler** — root, `/update`, restart, and not-found
  included; the auth check is the first line of every route. No endpoint
  exists without auth. Upload additionally requires the image signature
  (TM-O02) and anti-downgrade; auth and signature are both mandatory,
  neither substitutes for the other.
- **Fix phase:** Firmware/OTA (day-1, not deferred). Gate: endpoint-by-endpoint
  auth test in the rig.
- **Residual risk:** Low after the fix.

### TM-O02 — Tampering: firmware image swapped in transit

- **Attack:** an attacker swaps the image during OTA upload, or uploads an
  incomplete / corrupt image.
- **Impact:** High — bricked or compromised firmware.
- **Rust status: UNCHANGED.** Image signature **Ed25519, verified before
  apply** (existing keypair reused, fingerprint
  `387764a4…6411`); size/magic check (magic 0xE9, size cap = app
  partition); anti-downgrade (only equal or newer version accepted;
  version scheme redefined for the Rust series, not compared to C#
  v4.0.x); app verifies before upload (`ed25519-dalek`
  `verify_strict`, baked public key — the upload button is not even
  enabled until verification passes); board verifies again after upload.
  Stalled-upload DoS guard (C# Round 2 firmware fix) carried. Brick
  recovery doc + test carried.
- **Fix phase:** Firmware/OTA + UI/Tauri (upload UI) + Release (signing
  workflow, fail-closed).
- **Residual risk:** Low.

### TM-O03 — Information Disclosure / Brute Force: OTA password guessed

- **Attack:** an attacker guesses the WebOTA password (brute force /
  dictionary). If the AP password and the OTA password are the same
  credential, one guess opens both (OPEN-13/14 — see the discrepancy flag
  in KEY-LIFECYCLE-RUST §8: the design baseline is one credential per
  Architecture §2.5; SECURITY-INPUT §5 lists it undecided; it must be
  closed before pairing is implemented).
- **Impact:** High if a guess succeeds (= TM-O01).
- **Rust status: UNCHANGED, with the lockout rule locked.** Random
  per-device password, never MAC-derived; forced change on first setup
  (NIST 8259A); **lockout: 5 wrong attempts → 60 s lockout, for everyone**
  (C# values carried). The lockout is itself a repeatable lockout-DoS —
  **an accepted tradeoff, not a fix** (SEC-R2-05, carried); during lockout
  even the correct password is rejected, by design; the USER-GUIDE says so.
- **Fix phase:** Firmware/OTA (lockout + constant-time compare — SEC-R2-07
  is a day-1 fix in Rust, not "document only" as in C#).
- **Residual risk:** Medium-Low (SEC-R2-05 accepted tradeoff).

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

### TM-P01 — Information Disclosure: key stolen from the credential store (malware / other user)

- **Attack:** malware running as the same Windows user opens the keyring
  entry and extracts the device key; or the key leaks in plain text in a
  log / crash dump / settings export.
- **Impact:** High — the key gives full board control.
- **Rust status: SAME LIMIT, NEW MECHANISM.** `keyring` (Windows Credential
  Manager, DPAPI-backed) is user-bound, exactly like C# DPAPI — the
  **same** limit: malware as the same user can open it. Key never in a
  log / crash dump / telemetry / export / UI (KEY-LIFECYCLE-RUST §4.2;
  `tracing` log-redaction tests in CI; crash dumps opt-in, default OFF,
  redacted; telemetry default OFF, D31). No export / backup / cloud sync.
  **Honest limit:** no app-level design removes this; PC hygiene is the
  user's responsibility and the USER-GUIDE says so. If compromise is
  suspected, KEY_RESET replaces the key and ends the exposure.
- **Fix phase:** Engine/Protocol (`keyring` store, `zeroize` handling) +
  Release (docs, redaction gate).
- **Residual risk:** Medium. PC compromise = key compromise.

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

- **Attack:** the installer, logs, or a crash dump leaks the key /
  password / Wi-Fi password; or an old AppData folder keeps credentials
  after a migration.
- **Impact:** Medium-High.
- **Rust status: UNCHANGED rules, stricter in Rust.** No secret in logs —
  not plain, not hashed, not the first characters, not even the length
  (length is a leak too — do not log it). Secret-bearing types mask or
  omit secrets in `Debug`. Log files under `%AppData%\RSH\logs`,
  current-user-only permissions, rotation + size cap. Redaction is a CI
  test (log-output scan: 0 hits for password/key/token/secret/credential/
  auth), not a manual check. App is **unsigned** (OV cert deferred, D-R11)
  — SmartScreen warning expected; the release notes say so; users are
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
  applies a bad image. vJoy DLL is **never bundled** (D18) — resolved
  only from registry / Program Files, SHA-256 pinned; if missing, show a
  download link. SBOM (CycloneDX, Rust + frontend) + THIRD-PARTY-LICENSES.md
  with every release.
- **Fix phase:** Engine/Protocol (validation) + Firmware/OTA + Release (SBOM).
- **Residual risk:** Low-Medium (social-engineering a user into opening a
  bad file is always possible — USER-GUIDE warning).

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
  upload, KEY_RESET trigger, key export (if such a command existed),
  config writes.
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
  and a state check on every privileged command (upload, reset, pairing,
  config write: paired? window open? armed?).
  (d) **No secret ever reaches the frontend** — device key / Wi-Fi password
  / signing key are never returned to the frontend; the frontend shows
  only masked/status information ("paired ✓" — not even the last 2
  characters of a password).
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
  carried); vJoy DLL SHA-256 pinned, never bundled (D18); HIDMaestro
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
item is not hidden; "Decided" means this document (with
KEY-LIFECYCLE-RUST §3) records the decision and its reason.

| ID | Severity | Status | Fixing phase |
|---|---|---|---|
| **SEC-R2-01** — no in-app firmware upload in classic mode (wizard-only) | Medium | **FIX in Rust by design.** Tauri has no "classic mode / wizard skip": firmware upload lives in the **main window** (Settings/Diagnostics or a dedicated Firmware page, via a `tauri::command`), with the verify-first rule (TM-O02) — the upload button is not enabled until the image is verified. The wizard-only upload design is not rebuilt. Gate: upload end-to-end from the main UI. | UI/Tauri |
| **SEC-R2-03** — unpaired board's AP stays open indefinitely after the 60 s window | Medium | **DECIDED (§4 below): after the window the AP moves to WPA2 with the generated password** — not closed, not left open. Reason: closing the AP would strand the board (only KEY_RESET could recover it, punishing a user who merely missed the window); leaving it open indefinitely was the original defect. WPA2 keeps the board reachable for a later pairing attempt (password from the board label / USB-serial pairing) while removing the indefinite open-AP exposure. | Firmware/OTA (AP state machine) |
| **SEC-R2-04** — unauthenticated HELO `sessionFlag=1` rotates the nonce and resets both control sessions (DoS) | Medium | **Open — fix defined, not yet implemented.** Rate-limit new-session requests (e.g. one new session per N seconds, or only when no authenticated session is active). Exact limit + rig DoS test in the Firmware phase. DoS only, never control. | Firmware/OTA (with Engine) |
| **SEC-R2-06** — no board-binding enforcement in the key store (evil twin inside the 60 s window can overwrite the stored key/password) | Medium | **Open — fix defined, not yet implemented.** A HELLO key from a **different BoardMac** than the stored one requires **user confirmation** (confirm dialog in the frontend, with a Rust-command state check); without confirmation the stored credentials are **not** overwritten. The profile-level BoardMac warning from C# becomes a **gate**, not a warning. | Engine/Protocol |
| **SEC-R2-07** — WebOTA Basic-auth compare not constant-time | Low | **FIX in Rust firmware, day-1** (in C# this was "document only" because of the lockout; in Rust the constant-time compare is mandatory from the start). | Firmware/OTA |
| **SEC-R2-05** — WebOTA lockout is itself a repeatable lockout-DoS | Low | **Accepted tradeoff, documented** (TM-O03). No fix; USER-GUIDE records it. | — (accepted) |
| **SEC-R2-02** — legacy plaintext Wi-Fi password wipe lazy/not guaranteed | Low | **DROPPED with reason** — fresh series, no legacy plaintext store exists; password fields are excluded from any C# settings import. Reopens only if a C# migration is ever built. | — (closed) |
| **SEC-H-04** — key-compromise response, board-side execution | — | **CARRIED to Release.** The C# doc (`FLASH-ENCRYPTION-PROCEDURE.md` §7) is rewritten for the Rust series (new keypair/version scheme); board-side execution (rotation / compromise drill) happens at the A7-style real-board run. The doc review is not re-done here. | Release |
| **OPEN-3** — flash encryption + encrypted NVS, board-side | — | **CARRIED to Firmware/OTA + Release.** Encrypted NVS is a day-1 requirement for Rust firmware (Track B); for Track A (C++ v4.0.1) the key stays plaintext NVS until the eFuse burn + dump verification runs on a sacrificial board first. Closure criteria rewritten for the Rust series. Until then TM-W07 residual Medium stands, and the release notes say so. | Firmware/OTA + Release |
| **OPEN-6** — pairing bootstrap | — | **DECIDED (§4 below): wired USB-serial pairing is the primary bootstrap; the open-AP bootstrap is the fallback, with a USER-GUIDE warning.** Reason: a USB cable is a physical trust anchor an attacker cannot reach from Wi-Fi range, so on the primary path the one-time key disclosure never crosses the air; the open AP remains only as a fallback for users without a cable, and its residual risk (TM-W02) stays on the books rather than being designed away on paper. | Firmware/OTA (implementation) + UI/Tauri (pairing UX) |
| **OPEN-19** — key compromise response / revocation | — | **Carried as a document to be rewritten for the Rust series (SEC-H-04 above).** | Release |
| **OPEN-13/14** — one credential vs two (Wi-Fi vs OTA password); length/charset | — | **Discrepancy flag:** design baseline is one credential (Architecture §2.5; KEY-LIFECYCLE-RUST §8); SECURITY-INPUT §5 lists it undecided. Must be closed before pairing is implemented; the choice must not change silently in code. | Architecture/Firmware |
| **OPEN-4** — RNG readiness / key-gen failure behaviour | — | Rule carried (fail closed — KEY-LIFECYCLE-RUST §2); mechanism to be defined. | Firmware (Phase D) |
| **OPEN-5** — pairing-window start trigger | — | Carried — confirm AP-up + 60 s. | Firmware (Phase D) |
| **OPEN-11** — OTA does not touch key NVS | — | Carried — confirm in Firmware phase. | Firmware (Phase D) |
| **OPEN-12** — forced password-change UX | — | Carried. | UI/Tauri + Firmware |
| **OPEN-18** — anti-downgrade mechanism (Rust version scheme) | — | Carried — version scheme redefined for the Rust series. | Firmware/Release |

### Decisions recorded in this document (A5)

1. **SEC-R2-03 — WPA2 after the window.** After the 60 s pairing window,
   an unpaired board's AP moves to WPA2 with the generated password (not
   closed, not left open). Reason: closing strands the board; leaving it
   open was the defect; WPA2 keeps it reachable for later pairing while
   removing indefinite open exposure. (KEY-LIFECYCLE-RUST §3.)
2. **OPEN-6 — wired USB-serial pairing is the primary bootstrap; open-AP
   bootstrap is the fallback with a USER-GUIDE warning.** Reason: the cable
   is a physical trust anchor unreachable from Wi-Fi range, so the key
   never crosses the air on the primary path. (KEY-LIFECYCLE-RUST §3.)

Both decisions are frozen at A5 level: a later phase may implement them,
but may not re-decide them without amending these two documents first.

---

## 6. Residual-risk table (risks that remain after v1.0.0 — stated, not hidden)

| Risk | Level | Why it remains |
|---|---|---|
| TM-W02 HELLO sniff / TM-W01 evil twin (first pairing, open-AP fallback path) | Medium-High / Medium | No cryptographic fix exists for first identity before the board is known; the wired-primary decision narrows it to the fallback path only. |
| TM-W07 flash dump (OPEN-3) | Medium | eFuse burn + dump verification on a real board is still pending. |
| TM-P01 PC malware takes the key (keyring is user-bound) | Medium | The OS keyring is user-bound, not app-bound — PC compromise = key compromise. No app design removes this. |
| TM-R01 XSS → command abuse | Medium (mitigated) | CSP + capabilities + Rust-side validation reduce it; a compromised frontend dependency is always possible. |
| TM-R04 supply chain (build.rs / typosquat) | Medium-Low | Audit gates reduce it; a zero-day compromised crate has no complete fix. |
| TM-W05 deauth / RF DoS | Medium | No software fix; the wired transport is the answer. |
| TM-W04 stream tampering | Medium-Low | No HMAC on the stream (design decision). |
| TM-O03 OTA brute force (lockout-DoS tradeoff) | Medium-Low | SEC-R2-05 accepted tradeoff. |
| TM-R07 unsafe FFI boundary | Medium-Low | The boundary cannot be made memory-safe; it can only be contained and audited. |
| HIDMaestro Rust API unverified | Medium (phase gate) | Decision after verification — if no native API exists, vJoy-only v1.0.0 or a sidecar design. |
| TM-R05 old WebView2 on the user's PC | Low-Medium | Mitigated by the startup version check; the user must still apply the update. |

---

## 7. Traceability checklist — Architecture §2.5, rule by rule

Every rule in `RSH-RUST-ARCHITECTURE.md` §2.5 must trace to a line in
`KEY-LIFECYCLE-RUST.md` (KL) or this document (TM). Result: **all 9 rules
trace. Nothing could not be traced.** One discrepancy flag is recorded
(rule 4), not a missing trace.

| # | Architecture §2.5 rule | Traced to |
|---|---|---|
| 1 | Control channel HMAC-SHA256, per-device key, nonce + counter. No unauthenticated restart (`RSTX`) — only HMAC-signed REBOOT. | KL §1, §6; TM §2 TM-W03, TM-W08 |
| 2 | Per-device key: 32 B random, board first boot from hardware RNG; disclosed only in the 60 s pairing window in the first HELLO; PC stores in DPAPI/keyring, bound to board MAC; never plain/log/UI/export; FACTORY_RESET does not wipe it; KEY_RESET (BOOT-hold ≥ 10 s, armed 120 s) makes a new key + new password. | KL §2, §3, §4, §5 |
| 3 | Pairing bootstrap (OPEN-6, honest residual): AP open until first pairing; on first HMAC-signed GET_CAPS OK the AP becomes WPA2-PSK. No link-layer encryption in the window — proximity + 60 s; USER-GUIDE must direct pairing only at home/in your own room. | KL §3 (OPEN-6 decision + SEC-R2-03 decision); TM §4–§5 (decisions), §2 TM-W01/W02 |
| 4 | Wi-Fi/OTA password: **one credential** for both; 24 chars, charset `A–Z a–z 2–9` (57 symbols, ≈140 bit); display 4 groups of 6 hyphenated, wire/store without hyphens. SET_PASSWORD (8–63 chars, printable ASCII) HMAC-signed, mandatory. | KL §8; TM §3 TM-O03. **Discrepancy flag (not a missing trace):** SECURITY-INPUT §5 lists OPEN-13/14 (one vs two credentials) as undecided, while Architecture §2.5 states one credential. The design baseline is one credential; the Firmware phase must close OPEN-13/14 before pairing is implemented, amending both documents together if the answer changes. |
| 5 | Signed firmware (Must): **Ed25519**, verify **before apply**, anti-downgrade (version < running → reject). App verifies before upload (`ed25519-dalek`, verify-only). Signing key outside the repo, CI secret; public key baked in firmware. | KL §1, §12; TM §3 TM-O02 |
| 6 | Web OTA: HTTP Basic Auth on **every** handler, lockout, CSRF; ArduinoOTA disabled. | KL §12 (summary); TM §3 TM-O01, TM-O03, TM-O04 |
| 7 | Open items carried: SEC-R2-01, SEC-R2-03, SEC-R2-04, SEC-R2-06, OPEN-3, OPEN-19. | TM §5 table (all six, each with status + fixing phase); KL §14 |
| 8 | HELO acceptance (SEC-D-08): a second IP cannot overwrite `pcIp` in a live session; flap throttle 5 s; HELLO-reply rate limit 1 per 500 ms; stream only after at least one HMAC-verified control command was ACCEPTed in this session (SEC-E-14) — the app must send signed GET_CAPS first on every (re)connect. | TM §2 TM-W05 (rules stated in full) |
| 9 | App signing: app **unsigned** (OV cert deferred — user decision); SmartScreen warning expected. Firmware signing unaffected. | KL §12; TM §3 TM-P03 |

**Checklist result: 9 of 9 rules traced; 0 rules untraceable.** The single
flag is the OPEN-13/14 source discrepancy on rule 4, recorded above and in
KL §8 — it is a conflict between two source documents to be closed in the
Firmware phase, not a rule missing from these two documents.

---

## 8. Phase-wise mitigation map

| Phase | Threats |
|---|---|
| Foundation | TM-R03 (npm audit gate), TM-R04 (cargo audit/deny gates, lockfiles, deny.toml, secret-scan), TM-R01 (CSP + capability template), TM-P03 (log-redaction CI test) |
| Engine / Protocol | TM-W03, TM-W08, TM-W04 (vectors), TM-R02 (command validation), TM-R07 (FFI module), TM-P01/P03 (keyring store, zeroize), SEC-R2-06 fix, TM-P02 (profiles) |
| Firmware / OTA | TM-O01 (auth day-1), TM-O02, TM-O03, TM-O04, TM-W05 (rate limits), SEC-R2-03 (WPA2 after window), SEC-R2-04 (new-session gating), SEC-R2-07 (constant-time compare), OPEN-3 (encrypted NVS day-1 for Track B), OPEN-6 (bootstrap implementation) |
| UI / Tauri | TM-R01, TM-R02, TM-R05, TM-R06, TM-R08, SEC-R2-01 fix (upload in main window), OPEN-12 (forced password-change UX), OPEN-6 (pairing UX prefers wired) |
| Release | TM-O02 (signing workflow, fail-closed), TM-P03/P04 (SBOM, installer, docs), SEC-H-04 + OPEN-19 (rotation drill), OPEN-3 (eFuse burn + dump verification, sacrificial board first), TM-R05 (WebView2 bootstrapper decision), TM-R07 (FFI audit) |

---

*Doc type: design only. No mitigation in this document is code. If a phase
finds a mitigation unworkable, the mitigation is amended here first; it is
never changed silently in code. Open items are tracked in §5 and in the
residual-risk table in §6 — they are not hidden, and no Critical/High item
is open without a named fixing phase.*
