# Security Re-scan — Phase H1 (RSH Rust recreation)

- **Date:** 2026-11-14
- **Scope:** whole Rust codebase + Tauri frontend + reference firmware v4.0.1 (read-only) + dependency audit
- **Input:** `docs/PHASE-G-GATE.md` conditions 3, 8, 9, 10, 13, 15, 16; `docs/THREAT-MODEL-RUST.md`; `docs/KEY-LIFECYCLE-RUST.md`; `SECURITY-INPUT.md`
- **Verdict: CONDITIONAL PASS — 0 Critical, 0 High**
- **Conditions of the pass** (all recorded in §7, none is a live vulnerability):
  1. The 3 failing tests (§6) must re-run green on CI / Windows — they are sandbox-timing failures, proven environmental, not code defects.
  2. v1.0.0 cannot ship until the **session-owner activator** lands (§2, SEC-E-14): the gate is built and closed by default, so without the activator no input flows at all.
  3. The `cargo audit` yanked check timed out again (5th consecutive phase) — carried, not passed (§5).
  4. `npm audit` was blocked by the sandbox proxy — re-run on CI (§5).

## 1. STRIDE findings

Severity: **C**ritical / **H**igh / **M**edium / **L**ow. "Fixed in H1" means code + tests in this phase.

| # | STRIDE | Finding | Severity | File(s) | Disposition |
|---|--------|---------|----------|---------|-------------|
| H1-01 | Spoofing | **SEC-E-14: no HMAC session gate in the app.** `AppStateCore::forward_with_retry` forwarded stream frames as soon as a transport was up; `BoardSession` (which does the signed `GET_CAPS` first) had no owner in the app. Board-side the gate **is** enforced (firmware v4.0.1 `net.cpp`: `sessionAuthenticated`, stream emission gated in `stream_emit.h`), so with a genuine board no frames ever arrived — but a rogue board (evil twin, TM-W01/TM-W04) could have fed unauthenticated input. | M | `crates/rsh-app-core/src/state.rs` | **FIXED in H1** — `SessionGate` in `AppStateCore` (§2) |
| H1-02 | Spoofing | **SEC-R2-06: pairing overwrote a stored key without confirmation.** A `HELLO` disclosing a different key for an already-paired board silently replaced the stored key — an evil twin spoofing a board MAC could rebind the board. | M | `crates/rsh-transport/src/session.rs` | **FIXED in H1** — `StoredKeyExists` + `pair_with_replacement` (§3) |
| H1-03 | Spoofing | Sidecar hello identity check passed for **any** non-`None` `sidecar` value; a foreign process saying `{"sidecar":"anything"}` passed identification. | M | `crates/rsh-output-hidmaestro/src/protocol.rs` | **FIXED in H1** — exact `"rsh-hm-sidecar"` required (§4) |
| H1-04 | Tampering | Sidecar DLL-hash verdict was **optional**: a hello omitting both `dll_sha256_ok` and `dll_sha256` passed with no DLL verification at all; a measured hash contradicting the pin was ignored when the sidecar claimed `ok: true`. | M | `crates/rsh-output-hidmaestro/src/protocol.rs` | **FIXED in H1** — verdict required, measurement cross-checked (§4) |
| H1-05 | Info disclosure | Loopback token compared with `==` (timing leak of the matching prefix) and generated from a time+pid xorshift (predictable to a local attacker). | L | `crates/rsh-output-hidmaestro/src/transport.rs`, `src/protocol.rs` | **FIXED in H1** — OS CSPRNG (`getrandom`) + constant-time compare (`subtle`) (§4) |
| H1-06 | Elevation of privilege | `$RSH_HM_SIDECAR` overrode the sidecar path in production builds — anyone influencing RSH's environment could redirect the elevated (`runas`) sidecar launch at an arbitrary executable. | M | `crates/rsh-output-hidmaestro/src/manager.rs` | **FIXED in H1** — env override is debug-only (`cfg(debug_assertions)`) (§4) |
| H1-07 | DoS | Unbounded `read_line` on sidecar stdout/stderr: a compromised sidecar could exhaust RSH memory with one huge line. | L | `crates/rsh-output-hidmaestro/src/transport.rs` | **FIXED in H1** — 64 KiB line cap (§4) |
| H1-08 | Info disclosure | Tauri CSP lacked `script-src`, `object-src 'none'`, `base-uri 'self'`, `frame-ancestors 'none'`, `connect-src` (TM-R01). | L | `src-tauri/tauri.conf.json` | **FIXED in H1** — CSP tightened (§4) |
| H1-09 | Tampering | **SEC-R2-03:** unpaired board AP stays open indefinitely after the 60 s window expires — no code path moves it to WPA2. Residual is limited (after expiry `HELLO` discloses nothing, so no key leak; exposure is DoS/sniffing). Firmware change. | M | `reference/firmware-v4.0.1/net.cpp:230` (`keystore_unpaired()` at boot; WPA2 only after `keystore_mark_paired()`) | **DEFERRED to v1.0.1** with reason (§7) |
| H1-10 | DoS | **SEC-R2-04:** `net_new_session()` has no rate limit; any HELO with `sessionFlag=1` resets both control sessions immediately (also the serial path). The 500 ms HELLO-reply limit caps replies, not session resets. Firmware change. | M | `reference/firmware-v4.0.1/net.cpp:272,821` | **DEFERRED to v1.0.1** with reason (§7) |
| H1-11 | — | `cargo deny` warned `advisory-not-detected`: stale ignore `RUSTSEC-2024-0429` (glib 0.18.5) no longer matches any advisory in the DB. | L | `deny.toml:65` | **FIXED in H1** — stale ignore removed; deny green (§5) |
| H1-12 | — | Clippy (1.100) flagged two `implicit_saturating_sub` sites in the rig's trapezoid generators and a `large_types_passed_by_value` + `similar_names` in H1's own new code. | L | `tools/rsh-rig/src/wheel.rs`, `crates/rsh-app-core/src/state.rs`, `crates/rsh-engine/src/engine.rs` | **FIXED in H1** — clippy's own suggestions applied; clippy green (§6) |

### Checked and clean (no finding)

- **KeyStore / keyring** (`crates/rsh-security/src/keystore.rs`): keyring 4.2.0, service `RSH`, accounts `board:<MAC>` / `board:<MAC>:wifi`; hex 32 B key; fail-closed on Linux sandbox (no Secret Service); zeroization via volatile writes; `Debug` masks secrets. `get_wifi_password` has no production callers (tests only) — the Wi-Fi password never leaves the keystore in app code.
- **Diagnostics redaction** (`crates/rsh-app-core/src/diagnostics.rs`): `SECRET_KEYS` + `redact_secrets()`; bundles never contain secrets; tests plant secret values and assert redaction.
- **Firmware verify** (`crates/rsh-security/src/firmware_verify.rs`): Ed25519 verify-before-apply, `verify_strict`, baked pubkey fingerprint `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411` (matches SECURITY-INPUT.md §4.1), anti-downgrade, size/magic checks.
- **Updater:** `active: false`; the minisign pubkey **is** configured in `tauri.conf.json` (`RWRU7xMj5S9Eodnz8HNKEz8LacETwEFxkttjZLnCbopqmAdFYT2S7DGb`) — activation is H4 scope, not H1. (Earlier notes saying the pubkey was empty are stale; deep-scan Round 2 corrected this line — the previous value `RWRmxqPPEAQZxs40GGH34vn3bkWhDeMyRJyF3gSzq2MzIT639wXiTMED` matched no config or H4 doc and is removed.)
- **Capabilities** (`src-tauri/capabilities/default.json`): only `core:default`; no fs/shell/dialog/opener/updater/store/http. Good (TM-R06).
- **Frontend:** no `eval(` / `dangerouslySetInnerHTML` / `innerHTML` in `frontend/src/`; `dist/index.html` has one external module script and no inline scripts and no external URLs in built CSS/HTML — the CSP tightening in H1-08 is safe.
- **Unsafe/FFI:** confined to FFI modules (`vjoy.rs`, `launcher.rs`, `watcher.rs`) with `SAFETY` comments; zeroization uses `unsafe { write_volatile }` with `SAFETY` comments. No secrets cross FFI.
- **SEC-R2-02 (legacy plaintext store):** DROPPED with reason — fresh series, no legacy store exists.
- **SEC-R2-05:** accepted tradeoff, documented in the threat model.
- **SEC-R2-07:** day-1 fix in Rust firmware (firmware-side).

## 2. SEC-E-14 verdict — FIXED (app-side gate; activator pending)

**Verdict: the gate is fixed in H1. Shipping v1.0.0 without it was NOT acceptable; it is now enforced.**

- **What was missing (Phase G condition 13):** `BoardSession` implements the gate at session layer (`pair()`/`resume()` send signed `GET_CAPS` first; `stream_allowed() == is_active()`), but `AppStateCore` had no `BoardSession` owner, so the forwarder streamed frames as soon as a transport was up.
- **Fix — `SessionGate` in `AppStateCore`** (`crates/rsh-app-core/src/state.rs`):
  - Gate is a `Mutex<bool>` on the state, **default CLOSED**; `open_session_gate()` is the explicit API for the future session owner; `close_session_gate()` is called on disconnect and on transport loss (a new transport is a new session and must re-authenticate).
  - `forward_with_retry` checks the gate **first**, before `note_frame_encoders` — held frames never reach snapshots, encoders, or the engine — and counts them (`session_gate_held_frames`). `transport_state()` reports `session_active` + `frames_held_by_gate`, so a held stream is visible, never silent.
  - Full session activation inside the forwarder was rejected as infeasible without restructuring `UdpTransport` (`BoardSession`/`ControlClient` take the transport by value; stream + control share one socket demuxed into channels; a second socket cannot receive control replies).
- **Board-side (already enforced, verified in reference firmware v4.0.1):** `sessionAuthenticated` set only on HMAC-verified control ACCEPT (`net_ctrl_handler`); stream emission gated (`stream_emit.h`). With a genuine board the app receives no stream frames until it authenticates.
- **Remaining work (not H1):** the **session-owner activator** — a `BoardSession` owner that runs `pair()`/`resume()` and calls `open_session_gate()` on success. Until it lands, the gate stays closed and **no input flows at all** (fail-closed, but non-functional). v1.0.0 cannot ship before the activator lands. The pairing Tauri commands (`begin_pairing`, `get_pairing_status`, `set_password`) wait on the same owner (§7, condition 15).
- **Tests:** `session_gate_holds_frames_status_pump_and_disconnect` (gate closed → 10 frames held/counted, backend sees only neutral idle ticks; gate open → frames flow; disconnect → gate closes), plus the loopback test now opens the gate explicitly. The two UDP forwarder tests serialize on a port lock and the fake-board sender retries `ConnectionRefused` until the forwarder binds (both were races found while writing the gate test).

## 3. SEC-R2-06 verdict — FIXED (board binding)

`crates/rsh-transport/src/session.rs`: `pair()` now loads the stored key **before** storing anything; a stored key that differs from the disclosed one returns `SessionError::StoredKeyExists` with store and state untouched. `pair_with_replacement()` (+ `pair_bytes_with_replacement`) is the confirmed path, called only after the user confirms the replacement in the pairing UI. Same-key re-disclosure is idempotent. Tests: refuses a different stored key (old key survives, state stays `Paired`), replacement succeeds after confirmation, same-key re-disclosure succeeds.

## 4. Sidecar trust boundary + CSP (H1-03 – H1-08)

All in `crates/rsh-output-hidmaestro/` (small, security-only fixes — the backends' internals were otherwise untouched):

- `protocol.rs::validate_hello`: identity is now exact (at least one of `hello` / `sidecar` must be exactly `"rsh-hm-sidecar"`); the DLL-hash verdict is **required** (a hello with no verdict is refused) and a measured hash contradicting the pin fails even when the sidecar claims `ok: true`; the token echo is compared in constant time (`subtle::ConstantTimeEq`). Verified compatible: the real C# sidecar always emits both identity fields + `dll_sha256`/`dll_sha256_ok` (`tools/rsh-hm-sidecar/Program.cs`), and the stub emits both too.
- `transport.rs::generate_token`: 16 bytes from the OS CSPRNG (`getrandom` 0.3.4) instead of a time+pid xorshift; panics loudly if the OS RNG is unavailable rather than falling back to weak randomness. `getrandom`/`subtle` added as exact-pinned workspace deps (both already in `Cargo.lock`).
- `transport.rs`: 64 KiB line cap on the protocol reader (overlong line → EOF) and on the stderr drain (overlong line dropped); stderr ring stays capped at 64 lines.
- `manager.rs::resolve_sidecar_path`: `$RSH_HM_SIDECAR` is honored **only under `cfg(debug_assertions)`**; release builds always use the exe directory. (No env-mutating unit test was added: the codebase has no precedent for process-global env mutation in parallel tests; the gate is verified by inspection + the debug-build path still works in tests.)
- CSP (`src-tauri/tauri.conf.json`): `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; connect-src 'self' ipc: http://ipc.localhost`.
- Tests added: 4 protocol tests (wrong `sidecar` value fails, `sidecar`-only identification, missing DLL verdict fails, hash-contradicts-verdict fails) + 2 transport tests (overlong line → EOF, line at cap delivered). HIDMaestro: **124 tests pass** (56 unit + 20 backend + 28 manager + 12 mapping + 8 protocol).

## 5. Log scan + dependency audit

### Log / secret scan — 0 real secrets

Grep for `password=`, `api_key=`, `secret=`, `-----BEGIN ... PRIVATE KEY`, `AKIA...`, `ghp_...`, `sk-...` across `*.rs` / `*.ts` / `*.tsx` / `*.json` / `*.md` / `*.cs` / `*.toml` (excluding `target/`, `node_modules/`, `dist/`): **0 real secrets**. Only hit: test fixture `WIFI_PASSWORD: &[u8; 24] = b"Ab3dEf6hJk9mNp2rSt5vXy8z"` in `crates/rsh-transport/src/session.rs:812` (test-only constant, also used in keystore tests as `TEST_PASSWORD`). No secrets in `docs/`, `frontend/`, fixtures, or the sidecar C# code. Signing keys were never touched (read the Ed25519 fingerprint from docs only).

### `cargo audit` — advisory scan PASS, yanked check CARRIED (5th phase)

- `cargo audit --deny warnings --no-yanked --no-fetch`: **exit 0, 0 vulnerabilities, 626 crate dependencies, 1509 advisories loaded.**
- `cargo audit --deny warnings` (with yanked check): **timed out again** — 612 occurrences of `error: couldn't check if the package is yanked: registry: request could not be completed in the allotted timeframe` (log `~/tmp/h1-audit-yanked-final.log`). The scan aborted before a verdict; **do NOT claim the yanked check passed.** Carried to the gate/CI. `--offline` is not a supported flag for `cargo-audit`.
- `cargo deny check`: **exit 0 — advisories ok, bans ok, licenses ok, sources ok.** 28 `warning[duplicate]` lines (duplicate crate versions in the tree — pre-existing, informational). The stale ignore `RUSTSEC-2024-0429` was removed (H1-11); the `advisory-not-detected` warning is gone and no advisory re-appeared.
- `npm audit`: **blocked** — sandbox proxy 403 `policy_denied` on `POST https://registry.npmjs.org/-/npm/v1/security/audits/quick`. Not a pass/fail; re-run on CI.

## 6. Tests — results and the sandbox-timing caveat

- `cargo fmt --check`: **green.** `cargo clippy --workspace --all-targets --exclude rsh-app -- -D warnings`: **green** (after fixing H1-12).
- Workspace `cargo test --workspace --exclude rsh-app --no-fail-fast`: **746 passed, 3 failed, 3 ignored.** Per crate: `rsh-app-core` 123/125, `rsh-engine` 80/80, `rsh-transport` 108/110 (2 ignored), `rsh-output-hidmaestro` 124/124, `rsh-security` 30/30, `rsh-output` 93/93, `rsh-protocol` 60+18+10, `rsh-profiles` 49/50 (1 ignored), `rsh-telemetry` 32+7, `rsh-rig` 9/9, benchmarks 1+1 (incl. 60 s soak).
- Frontend `npm test`: **207 passed, 6 failed** in the full run — all 6 were `Test timed out in 5000ms` in screen tests (`PinMapEditor` ×3, `Settings` ×2, `Tuning` ×1) while the cargo workspace suite ran concurrently on 1.5 shared CPUs. Re-run alone, those 3 files pass **40/40**; `tauri.test.ts` (covers the file H1 touched) passed in the full run. H1's only frontend change is two optional fields on the `TransportState` interface (type-level, erased at runtime).

### The 3 Rust failures are sandbox-timing, not code defects

All three are rate/pacing assertions. Each was reproduced **alone in a fresh process on a quiet machine**:

| Test | Expected | Measured alone |
|---|---|---|
| `engine_idle_without_transport_sends_no_fake_frames` (`rsh-app-core`) | loop rate > 150 Hz | 79–102 Hz |
| `snapshot_emitter_runs_at_30hz_without_blocking_ingest` (`rsh-app-core`) | ~30 snapshots / 1 s | 23 |
| `sustains_200_pps_for_two_seconds` (`rsh-transport`) | 400 frames / 2 s | 361 |

Control experiment: a bare tokio 5 ms interval on a `current_thread` runtime in this sandbox fires at **85 Hz** (`~/tmp/ticktest`, removed after the run) — the sandbox vCPU/timer cannot sustain 200 Hz tokio intervals. H1 changed no pacing code (the per-tick additions are a ~7 KB struct copy + a mutex lock, microseconds). **These assertions were NOT weakened** — weakening a Phase G gate test to fit the sandbox would mask real regressions on real hardware. They must re-run green on CI / Windows; the `engine_spawn_flows_frames_to_backend_watchdog_neutral_and_bounded_stop` failure seen once in a serial run (`frames processed: 2` vs ≥ 3) is the same cause.

## 7. Open-items disposition — nothing left unowned

| Item | Disposition |
|---|---|
| **Cond 11** — per-tick metrics outlet | **CLOSED.** `OutputSink::send_with_metrics(state, metrics)` (default delegates to `send`); engine `send_output` calls it; `EngineSink` feeds live metrics; `ingest_engine_output` now takes `&EngineMetrics` (clippy: 7.2 KB struct, no longer by value). Test: `per_tick_metrics_reach_the_snapshot` (jitter 0.1 ms / latency 0.2 ms land in the snapshot) + engine test `per_tick_metrics_outlet_reaches_the_sink`. |
| **Cond 12** — status pump | **CLOSED.** `PacketSource::try_next_status()` (default `None`); UDP + serial override via `status_rx.try_recv()`; forwarder drains after every frame into the engine's status sender; engine already had `status_sender()` + per-tick `drain_status`. Covered by the gate test (seq_gap 7 / tx_fail 3 reach the snapshot). |
| **Cond 13** — HMAC session gate (SEC-E-14) | **CLOSED as a gate; activator pending.** §2. |
| **Cond 14** — per-module tunings → pipeline | **DEFERRED to v1.0.1.** `ProcessingPipeline::set_plan` exists and the profile store holds per-module tunings, but `WheelEngine` has no plan-update channel — the fix needs an engine API addition + conversion + supervisor wiring, a medium feature on the hot path, with **no security impact** (the default plan is safe, just untuned). |
| **Cond 15** — 19 unregistered Tauri commands | **PARTLY CLOSED by H3, remainder deferred.** H3 registered 5 of the 19 (`get_transport_state`, `connect_transport`, `disconnect_transport`, `switch_transport`, `open_config_folder`; 42 commands total now) on both sides — Rust `main.rs` + `frontend/src/lib/tauri.ts` (`IMPLEMENTED_COMMANDS` 42, tests assert the transport commands are implemented). H1 verified this and mirrored the two new gate fields (`sessionActive`, `framesHeldByGate`) into the frontend `TransportState` interface. **Remaining 14 deferred to the session work:** pairing (3) needs the `BoardSession` owner; config staging (6), pin-map get/set (2), firmware upload/version (2), `restart_wheel` (1) need a live board connection + session. Registering them without backing would be fake implementations. |
| **Cond 16** — corrupt-store repair UI | **DEFERRED to v1.0.1 / H3.** Fail-closed `exit(1)` in `build_app_state` stands; backups survive; `restore_backup` + `list_backups` commands already exist; the repair UI is frontend work (H3 scope). |
| **Cond 8** — vJoy DLL SHA-256 | **CARRIED as release gate.** Windows-only measurement; cannot be done in the sandbox; never invent the hash (`PINNED_DLL_SHA256_HEX` is deliberately `None`, `verify_dll_sha256()` is ready). |
| **Cond 9** — pid.codes PR for 0x5253 | **CARRIED as release gate.** Needs the owner + a GitHub action; cannot close from the sandbox. |
| **Cond 10** — H-HID-1–6 owner-PC | **CARRIED.** Windows-only verification; deferred. |
| **SEC-R2-03 / SEC-R2-04** | **DEFERRED to v1.0.1** (firmware changes; H1-09/H1-10). Residuals are DoS/sniffing only, no key leak; the pairing-window expiry already stops credential disclosure. |
| **SEC-R2-06** | **FIXED in H1** (§3). |
| **OPEN-3/19 + SEC-H-04** | **CARRIED to Release.** Board-side eFuse burn + dump verification on a sacrificial board + doc rewrite for the Rust series; cannot be done in the sandbox. |
| **OPEN-13/14** — one credential Wi-Fi+OTA | **CARRIED.** App-side is consistent with the one-credential baseline (single `board:<MAC>:wifi` keyring entry, no second credential anywhere in code); firmware-side closure is needed before the pairing UI. |

## 8. Files changed in H1

- `crates/rsh-app-core/src/state.rs` — SessionGate, `TransportState` gate fields (merged into the existing Phase G struct — the first draft accidentally redefined it; fixed by merging, `kind_id` restored), `transport_state()`, conditions 11/12 hooks, forwarder gate + status drain, tests (gate/status/metrics; port-lock + bind-retry hardening).
- `crates/rsh-engine/src/engine.rs` — `OutputSink::send_with_metrics`, `PacketSource::try_next_status`, `send_output` uses the metrics outlet, test sink + test.
- `crates/rsh-transport/src/udp.rs`, `src/serial.rs` — `try_next_status` overrides.
- `crates/rsh-transport/src/session.rs` — SEC-R2-06 (`StoredKeyExists`, `pair_with_replacement`, `pair_bytes_with_replacement`, 3 tests).
- `crates/rsh-output-hidmaestro/src/protocol.rs`, `src/transport.rs`, `src/manager.rs`, `Cargo.toml` — sidecar fixes (§4) + 6 tests.
- `Cargo.toml` — `getrandom =0.3.4`, `subtle =2.6.1` workspace pins.
- `src-tauri/tauri.conf.json` — CSP tightened (single-line layout preserved).
- `src-tauri/src/main.rs` — doc correction: the session gate now exists; pairing commands still wait on the `BoardSession` owner.
- `frontend/src/lib/tauri.ts` — `TransportState` gains optional `sessionActive` / `framesHeldByGate`.
- `deny.toml` — stale `RUSTSEC-2024-0429` ignore removed.
- `tools/rsh-rig/src/wheel.rs` — clippy `saturating_sub` (mechanical, semantics-preserving).
- **Not touched:** `~/workspace/projects/rsh/RSH-handover/`, `~/workspace/projects/rsh/signing-keys/` (fingerprint from docs only). No git commands, no push, no zips. `src-tauri` does not compile in the Linux sandbox (WebKitGTK) — its changes mirror existing patterns exactly and are for Windows CI to compile.
