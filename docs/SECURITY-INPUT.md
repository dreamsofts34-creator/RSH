# RSH — Security Input (Rust series, v2.0.6)

**Status:** INPUT — the source decisions this series builds on,
restated for Rust. This document decides nothing new: it records the
inputs the threat model (`THREAT-MODEL-RUST.md`) traces to. Where it
disagrees with it, the disagreement is flagged, not hidden.

**Date:** 2026-11-20 (reconstructed 2026-11-14, updated for v2.0.6 —
pairing removed; see the note below)
**Series:** App Rust **v2.0.6** · Firmware **C++ v4.0.3** · Protocol
**v2.2 wire FROZEN**.
**Authority:** `ARCHITECTURE.md` §2.5 (security model) ·
`PROTOCOL-V2.2.md` (wire authority) · `THREAT-MODEL-RUST.md` §5
(open decisions).

> **Reconstruction note (deep-scan Round 2, 2026-11-14, updated
> 2026-11-20):** this file was missing from the repo while
> `THREAT-MODEL-RUST.md` cited it — the reference was dangling. It
> was reconstructed here from the citing documents, with the
> original section numbers preserved so the existing references
> (§1, §2.2, §4.1, §5, §6.1, §7.1, §7.2, §8, §9) resolve again.
> **v2.0.6 update:** pairing was abolished (user order 2026-11-20).
> The key lifecycle (§2) is replaced by the password-only lifecycle;
> the device-key store, the pairing bootstrap (OPEN-6) and the
> one-time pairing password carve-out (§7.2) are dead. Section
> numbers are kept so old references still resolve; dead sections
> say so instead of being silently renumbered. If a future reader
> finds the C# original, diff it against this file and record any
> drift — do not silently overwrite either one.

---

## 0. Scope — what this document is

The security inputs for the Rust series: the threat list, the key
lifecycle, the signing rules, the open decisions, and the carried
items — in one place, so the design documents can cite a section
number instead of re-arguing the input.

---

## 1. Threat model (TM-R01–R08)

The eight new Rust/Tauri threats, modelled in `THREAT-MODEL-RUST.md`
§4 (STRIDE, with attack, mitigations, fixing phase, residual risk).
C# threat IDs (`TM-Wxx`, `TM-Oxx`, `TM-Pxx`) are kept, never
renumbered; the Rust additions are:

| ID | Threat | Severity |
|---|---|---|
| TM-R01 | XSS → command abuse (WebView treated as hostile; CSP + capabilities + Rust-side re-validation) | High |
| TM-R02 | IPC input validation gap (Tauri commands deserialize JSON) | High |
| TM-R03 | Frontend supply chain (npm dependencies are build-time code) | Medium |
| TM-R04 | Rust supply chain: build.rs / proc-macro / typosquat | Medium |
| TM-R05 | WebView2 version / patching (OS-provided Chromium) | Low-Medium |
| TM-R06 | Tauri plugin over-grant (wrong capability hands out fs/shell) | Medium |
| TM-R07 | Unsafe FFI boundary (vJoy C DLL, HIDMaestro native API) | Medium |
| TM-R08 | Single-instance / autostart | Low |

No Critical/High threat is left open without a plan
(`THREAT-MODEL-RUST.md` §5–§6).

---

## 2. Password lifecycle (no key anymore — v2.0.6)

There is **no device key**. The only credential is the **user-set
Wi-Fi password** (8–63 printable ASCII, set in Settings → Connection
after first connect): it is the WPA2-PSK for the board's AP **and**
the WebOTA password (one credential, OPEN-13/14 closed). The board's
AP is **open** until the user sets a password; WebOTA is **disabled**
until a password is set. The password is stored on the PC only in the
OS credential store, never exported, never logged, and cleared only
by a physical `KEY_RESET` — hold BOOT ≥ 10 s and the firmware deletes
the NVS password itself and reboots to an open AP
(`PROTOCOL-V2.2.md` §5). No factory, no shared default password, no
cloud account, no key escrow. DTLS is deferred by decision — local AP
+ WPA2 password is the design, not an oversight. There is no HMAC and
no replay window in protocol v2.2 (PROTOCOL-V2.2.md §0, §2.3).

### 2.2 Keyring storage (D-R9)

- PC store: OS credential store via the `keyring` crate
  (Credential Manager, DPAPI-backed on Windows).
- Service `RSH`; account `board:<boardMac>:wifi` (Wi-Fi/OTA password).
  The device-key account `board:<boardMac>` is **deleted** — the app
  deletes every device-key entry at daemon startup on first v2.0.6
  run, and `:wifi` entries are deleted once the board reports
  firmware ≥ 4.0.3 (PROTOCOL-V2.2.md §8).
- In-memory: `zeroize` / `ZeroizeOnDrop`, never `String`, never
  logged, never in frontend IPC (§7.2).
- The C# DPAPI blob is unreadable from Rust: every user re-pairs
  (no migration of the C# key) — **dead with pairing**; the v2.0.6
  migration is app-first, then firmware 4.0.3 via WebOTA with the old
  stored Wi-Fi password (PROTOCOL-V2.2.md §8).

### 2.3 Board-side obfuscation

The board password is **obfuscated in NVS with a fresh random NVS key
generated once at first 4.0.3 boot** (never the deleted device key);
no encryption is claimed. Encrypted NVS is a day-1 requirement for
Rust firmware in Track B (esp-rs) instead (OPEN-3 carried).

---

## 4. OTA signing

- Firmware images are signed **Ed25519**, verified **before apply**,
  with anti-downgrade (version < running → reject).
- The existing C# keypair is **reused** — no new keypair (D-R10).
  The private key lives outside the repo
  (`~/workspace/projects/rsh/signing-keys/`, 0600) and in the CI
  secret `RSH_SIGNING_KEY_PEM`; the public key is baked into firmware.

### 4.1 Firmware signing public key (fingerprint)

SHA-256 of the firmware signing public key:

`387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`

Matches `crates/rsh-security/src/firmware_verify.rs`. (This is a
fingerprint, not a key — it
identifies the keypair; the public key itself is baked into the
firmware and the app's verifier.)

---

## 5. Open decisions

| ID | Question | Status |
|---|---|---|
| OPEN-13/14 | One credential vs two (Wi-Fi vs OTA password); length/charset | **CLOSED (v2.0.6): one credential** — the user-set Wi-Fi password (8–63 printable ASCII) is the WebOTA password too. The old discrepancy (Architecture §2.5 said one credential, this section listed it undecided) is resolved in favour of one credential; the choice must not change silently in code. |
| OPEN-6 | Pairing bootstrap | **DEAD with pairing (v2.0.6).** There is no bootstrap: first connect is an open AP, then the user sets a password in Settings. |
| OPEN-4 | RNG readiness / failure behaviour | Rule carried (fail closed); mechanism to be defined (Firmware phase) — now for the NVS obfuscation key generated at first 4.0.3 boot, not for a device key. |
| OPEN-3 | Flash encryption + encrypted NVS, board-side | Carried to Firmware/OTA + Release (see §8). |
| OPEN-19 | Password compromise response / revocation | Carried as a document to be rewritten for the Rust series (SEC-H-04, Release phase). |

---

## 6. Key separation

### 6.1 Three different keys (locked, D-R10)

The firmware signing key, the Tauri updater signing key, and the
(deferred) OV code-signing certificate are **three different keys**:

- **Firmware signing:** Ed25519, existing keypair reused (§4.1).
- **Tauri updater:** Ed25519, a NEW dedicated keypair for the Rust
  series (`tauri signer generate`); private key in the GitHub Actions
  secret `TAURI_SIGNING_PRIVATE_KEY`; public key baked in
  `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`,
  `RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb`).
  Rotate-new-pubkey-first; opt-in default OFF.
- **OV code-signing certificate:** deferred by user decision (D-R11);
  the app ships **unsigned**, SmartScreen warning expected. Firmware
  signing is unaffected.

---

## 7. Boundaries

### 7.1 FFI boundary

Only axis/button/report data crosses the FFI boundary (vJoy C DLL,
HIDMaestro native API) — no secret ever crosses FFI. Every value
coming back from FFI is validated Rust-side; a C DLL return is never
blindly trusted (`THREAT-MODEL-RUST.md` TM-R07).

### 7.2 In-memory handling

Secrets are held in zeroizing types (`ZeroizeOnDrop`), never as
plain `String`, never logged, never in a crash dump / telemetry
payload / export / UI, and never in frontend IPC — **no exception**.
The one-time pairing password carve-out that used to live here is
**dead with pairing** (v2.0.6): there is no `pairing_status()`
disclosure, no single-use consume flag — nothing is disclosed to the
frontend, full stop (THREAT-MODEL-RUST.md TM-R01(d)).

---

## 8. Carried items (from C#, status unchanged unless noted)

- **SEC-R2-01** — in-app firmware upload: FIX in Rust by design
  (main-window upload, verify-first; no wizard-only classic mode).
- **SEC-R2-03** — unpaired board AP open indefinitely after the 60 s
  window: **DEAD with pairing (v2.0.6).** No window, no "unpaired"
  state — the AP is open until the user sets a password, by design
  (THREAT-MODEL-RUST.md TM-W02). Successor: the first-connect
  password prompt + WebOTA disabled until a password is set.
- **SEC-R2-04** — unauthenticated HELO `sessionFlag=1` resets both
  control sessions (DoS): open, fix defined (rate-limit new-session
  requests), Firmware phase. Redefined for v2.2: no nonce exists to
  rotate, so the surface is HELO flood / session churn.
- **SEC-R2-06** — no board-binding enforcement in the key store:
  **DEAD with pairing (v2.0.6).** No stored key to overwrite. Board
  binding survives as the `BoardMismatch` gate (HELLO `board_mac`
  must match the joined SSID suffix).
- **SEC-R2-07** — WebOTA Basic-auth compare not constant-time:
  FIX in Rust firmware, day-1.
- **SEC-R2-05** — WebOTA lockout is itself a repeatable lockout-DoS:
  accepted tradeoff, documented (`THREAT-MODEL-RUST.md` TM-O03).
- **SEC-R2-02** — legacy plaintext Wi-Fi password wipe: DROPPED —
  fresh series, no legacy store exists.
- **SEC-H-04 / OPEN-19** — password-compromise response, board-side
  execution: carried to Release (document rewrite + password-clear
  drill: BOOT-hold 10 s → open AP → set new password, at the
  A7-style real-board run).
- **OPEN-3** — flash encryption + encrypted NVS: carried to
  Firmware/OTA + Release; until the eFuse burn + dump verification
  runs on a sacrificial board, TM-W07 residual Medium stands.
- **OPEN-11** — OTA does not touch password NVS: carried, confirm in
  the Firmware phase (the 4.0.3 first-boot secret wipe is a one-time
  migration, not an OTA behaviour).
- **OPEN-5** — pairing-window start trigger: **DEAD with pairing
  (v2.0.6).** No window exists.

---

## 9. Phase map

| Phase | Security work |
|---|---|
| Foundation | TM-R03 (npm audit gate), TM-R04 (cargo audit/deny gates, lockfiles, `deny.toml`, secret-scan), TM-R01 (CSP + capability template), TM-P03 (log-redaction CI test) |
| Engine / Protocol | TM-R01, TM-R02 (per-command validation), TM-W01 (BoardMismatch gate), SEC-E-14 (session gate — plain GET_CAPS OK) |
| Firmware / OTA | SEC-R2-04, SEC-R2-07, OPEN-3, OPEN-4, OPEN-11, TM-W02 (WebOTA disabled until password set) |
| UI / Tauri | TM-R01, TM-R02, TM-R05, TM-R06, TM-R08, SEC-R2-01 fix, OPEN-12 (first-connect password prompt) |
| Release | SEC-H-04 / OPEN-19 rewrite + drill, FFI audit (TM-R07), capability review at the release gate, app-signing decision (OV deferred) |

---

## 10. Residual risks

Carried from `THREAT-MODEL-RUST.md` §6 (no Critical/High left open
without a plan; the rest is stated honestly, not designed away):

- Open-AP window before a password is set (TM-W02): Medium-High —
  anyone in range can join and set a password first; bounded by the
  first-connect prompt, not by cryptography.
- Unauthenticated control for anyone on the AP (TM-W08): High on an
  open AP / Medium-Low once a password is set — no HMAC by design.
- Control replay, reqId dedup only (TM-W03): Medium — accepted
  LAN-only risk.
- Flash dump (TM-W07, OPEN-3): Medium until the eFuse burn + dump
  verification runs on a real board.
- PC malware takes the password (TM-P01): Medium — the OS keyring is
  user-bound, not app-bound; PC compromise = password compromise.
- XSS → command abuse (TM-R01): Medium (mitigated) — a compromised
  frontend dependency is always possible.
- Supply chain (TM-R04): Medium-Low — a zero-day compromised crate
  has no complete fix.
- Stream tampering (TM-W04): Medium-Low — no HMAC on the stream, by
  design decision.
- OTA brute force / lockout-DoS tradeoff (TM-O03, SEC-R2-05):
  Medium-Low, accepted. Strength now also depends on the user's
  chosen password.
- Unsafe FFI boundary (TM-R07): Medium-Low — contained and audited,
  never memory-safe.
- Old WebView2 on the user's PC (TM-R05): Low-Medium — mitigated by
  the startup version check; the user must still apply the update.

---

## 11. Windows / human gates (added deep-scan Round 2, 2026-11-14)

Checks no Linux sandbox or CI job can settle — a Windows PC (or
Windows CI) with a human in the loop must close them before the
release they block:

| Gate | Check | Blocks |
|---|---|---|
| W-1 | **Dialog plugin compile verification** — `src-tauri` with `tauri_plugin_dialog::init()` + `dialog:default` must compile on Windows CI. The Linux sandbox cannot compile `rsh-app` at all (`gdk-sys` missing), and the Windows-target check fails at the `ring` build script (no MSVC `lib.exe` in the sandbox). Resolution was verified (`cargo tree` shows `tauri-plugin-dialog 2.8.4`; `init()` and `dialog:default` were verified against the downloaded crate source), but no Rust code in `rsh-app` was type-checked here. | v1.0.0 release — **CLOSED by shipped releases** (v2.0.x CI green on Windows; the `tauri.conf.json` `"dialog": {}` init bug that blocked v1.0.2 was fixed in the v2.0 redesign). |
| W-2 | **Foreground-detector end-to-end** — a known game in the foreground on a Windows PC must return that game's id from `WindowsForegroundDetector::foreground_game()`. The property test `windows_foreground_detector_result_is_a_known_game_id` (runs on Windows CI, no game needed) only proves the detector never returns an unmapped id. | **DEAD (2026-11-18): game auto-scanning was fully removed by user decision** — the user adds games manually; there is no auto-switch to be confident about. |
