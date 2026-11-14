# Diagnostics + Crash Dumps (SW-R7.6)

> **Scope:** diagnostics export (implemented + tested) and crash-dump
> capture (approach + Windows-only parts documented here). User
> consent: telemetry is default OFF (D31) and the first diagnostics
> export asks the user — an export is never sent anywhere by the app;
> the user chooses where the file goes.

## 1. Diagnostics export (implemented)

**Code:** `crates/rsh-app-core/src/diagnostics.rs`
(`export_diagnostics`, `build_bundle`, `collect_log_tail`,
`redact_secrets`, `install_panic_hook`).

**Bundle** (`DiagnosticsBundle`, pretty JSON):
- `appVersion`, `os`, `arch`, `generatedUnixSecs`;
- `logTail`: the last 200 lines of `%AppData%\RSH\logs\rsh-*.log`
  (the S-11 daily-rotation name, `rsh-YYYYMMDD.log`), oldest file
  first, newest line last;
- `notes`: free-form notes (e.g. "exported from Settings →
  Diagnostics").

**What is deliberately NOT in the bundle:** the settings file (it
can hold the Wi-Fi password), any keystore content, device keys,
HMAC keys, full logs, profiles, or crash dumps. The tail is capped
at 200 lines so an export stays emailable.

**No-secrets rule (defence in depth, two layers):**
1. The logger itself must never write secrets (S-11 rule — the
   export assumes the log is clean and still verifies).
2. `redact_secrets` scrubs every log line and every caller-supplied
   note before it enters the bundle. The serialised JSON is
   deliberately *not* re-processed after serialisation — a
   line-based redaction pass over JSON would truncate values and
   corrupt the document (found by the export test: re-redacting
   broke the closing quote of a redacted value).
   (`password`, `passwd`, `secret`, `token`, `api-key`/`api_key`,
   `hmac`, `private`, `credential`, `bearer`, `auth`, `wifi`,
   `device_key`, `signing`) appears before a `=`/`:` separator
   (allowing the word `key` between key and separator, so
   `device hmac key: …` redacts while `author: …` does not — the
   bare word `key` is intentionally not a secret key, it appears in
   benign contexts like "key binding"). A bare "private key" line
   with no separator is redacted in full.

**Tests (in `diagnostics.rs`):** redaction of `=`/`:`/JSON
spellings, benign lines left alone (`author:`, `key binding:`),
log-tail capping, missing log dir → empty tail (not an error —
diagnostics must work on a broken install), and the export test
that plants fake `wifi_password` / `api_key` / `token` values in a
log and asserts **none** of them appear in the written file while
`[REDACTED]` does. A failed write (unwritable path) is an error,
not a silent empty file.

**Log locations** (for the support flow, not code): primary
`%AppData%\RSH\logs\rsh-YYYYMMDD.log`, fallback `%TEMP%\RSH\logs`
(C# v2.0.4 lesson — when the primary dir is unwritable).

## 2. Crash-dump capture (approach)

Two layers, because a Rust panic and a native crash are different
failures:

### 2a. Rust panic hook — implemented, portable

`install_panic_hook(log_dir)` (`diagnostics.rs`) installs a
`std::panic::set_hook` that appends
`panic at <file>:<line>:<col>: <payload>` to
`%AppData%\RSH\logs\diagnostics-panic.log`. The hook never panics
itself (a failed write is dropped) and redacts the line. A panic is
a bug, never a normal exit — the trace survives even when the
process dies before flushing anything else. Tested with
`catch_unwind` (hook installed, panic raised, previous hook
restored, file asserted redacted).

### 2b. Native Windows crash dump — documented design, NOT implemented

A native crash (access violation in the vJoy/HIDMaestro FFI layer,
a WebView2 crash) never reaches the Rust panic hook. The design:

- **Mechanism:** `SetUnhandledExceptionFilter` at startup
  (Windows-only, `src-tauri`, `windows` crate) → in the filter,
  `MiniDumpWriteDump` (`dbghelp`, `windows` feature
  `Win32_System_Diagnostics_Debug`) with
  `MiniDumpWithDataSegs | MiniDumpWithHandleData` (no full memory —
  a full dump can contain secrets from RAM) → write to
  `%AppData%\RSH\crashes\rsh-YYYYMMDD-HHMMSS.dmp`.
- **Why not `WER` alone:** Windows Error Reporting uploads to
  Microsoft and the user never sees the file; RSH needs a local
  file the user can attach to a support report, with consent.
- **Consent:** a dump is written only after the user has consented
  to diagnostics (first-export prompt, D31); without consent the
  filter still runs (to park the wheel — see §3) but writes no dump.
- **Secrets:** minidumps are scrubbed the same way as logs are not
  — a dump *can* contain in-memory secrets, so the export flow must
  warn the user before they share a `.dmp` ("may contain sensitive
  data — share only with RSH support"). No automated scrubbing of a
  binary dump is attempted; that would be a false promise.
- **Gate:** Phase H / Windows CI — needs the `windows` crate
  `dbghelp` feature, a forced-crash test on the owner PC
  (deliberate null-deref behind a hidden flag is *not* shipped;
  the test triggers it from a test-only build), and a check that a
  dump is produced and opens in WinDbg. Not compiled in this phase:
  the `windows` crate does not compile on Linux, and an untested
  filter is worse than no filter (a bad filter can hide crashes).

## 3. What happens on a crash (F4-adjacent)

A crash is an exit the app did not plan, so the shutdown-neutral
path (see `docs/SYSTEM-INTEGRATION.md` §6) cannot run. Two
mitigations, in order:
1. The **engine watchdog** already parks output to neutral when
   data goes stale — but a crashed process sends nothing at all.
2. The **output backends** are the real backstop: vJoy /
   HIDMaestro hold their last state until the device is released;
   the OS releases the device handle when the process dies, and the
   virtual device then reports neutral. This is driver behaviour,
   not app behaviour — it is verified on the owner PC as part of
   the crash-dump gate (§2b), not assumed here.

## 4. Support flow (for the user guide, not code)

Settings → Diagnostics → "Export diagnostics" → the app writes
`rsh-diagnostics-<date>.json` where the user chooses → the user
attaches it to their report. The file contains versions + a redacted
log tail only; if the user also has a `.dmp` in
`%AppData%\RSH\crashes`, the UI offers to attach it with the
sensitivity warning from §2b.
