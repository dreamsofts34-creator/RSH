# RSH Rust — Spike Decisions Freeze (Phase A3)

> **Status: FROZEN 2026-11-14** (spike phase A3 — documentation + verification only, no production code).
> **Scope:** S-4 / S-5 / S-6 / S-10 / S-11 / S-13 + Tauri version check + data-dir + date/time/misc.
> **Method:** every version below was read live from crates.io / docs.rs / the crate source on
> 2026-11-14 (not from memory), and every API claim was compile- or run-checked in a throwaway
> spike project in `/tmp/a3-spike` (sources in `/tmp`; build target dirs under `~/.cache`
> because this sandbox's `/tmp` is a 512 MB tmpfs and the `windows`-crate build alone exceeds it).
> Linux sandbox: Windows-only *runtime* behaviour could not be measured here — it is marked
> **OPEN (Windows)** wherever it applies, never claimed as measured.
> **Pin rule:** exact `=x.y.z` pins go into `[workspace.dependencies]` + `Cargo.lock` when the
> Phase C / SW-R1 worker adds them. This file does **not** change any `Cargo.toml` itself.

## 0. Frozen decisions — summary

| # | Decision | Pinned version (verified 2026-11-14) |
|---|---|---|
| S-4 | Serial: **`serialport`** primary; `serial2` fallback | `serialport =4.10.1`, `serial2 =0.2.39`, `tokio-serial =5.5.0` (bridge, if needed) |
| S-5 | Crypto: **`hmac` + `sha2`**; CRC **hand-rolled** (A1 stands); **`ed25519-dalek`** verify-only | `hmac =0.13.0`, `sha2 =0.11.1`, `ed25519-dalek =3.0.0` (MSRV 1.85) |
| S-6 | Keystore (CRITICAL): **`keyring` 4.x, DEFAULT features** — never 3.x without `windows-native` | `keyring =4.2.0` (default `v1` feature; MSRV 1.88) |
| S-10 | Telemetry: **`windows` crate, `cfg(windows)`-gated**, `default-features = false` | `windows =0.62.2` (features below); fallback `windows-sys =0.61.0` |
| S-11 | Logging: **`tracing` + `tracing-subscriber` + custom daily-rotation writer** (tracing-appender CANNOT make the C# filename — see §5) | `tracing =0.1.44`, `tracing-subscriber =0.3.23`, `tracing-appender =0.2.5` (fallback only) |
| S-13 | Testing: **`criterion`** primary, **`proptest`**, **`cargo-fuzz` + nightly**, **`mockall`** | `criterion =0.8.2`, `proptest =1.12.0`, `cargo-fuzz =0.13.2`, `mockall =0.15.0` |
| Tauri | **A2 pin confirmed current** — no change | `tauri =2.12.5` (still latest stable 2.x today) |
| Data dir | **`dirs::data_dir() + "RSH"`** (A2 choice frozen) | `dirs =7.0.0` |
| Misc | **`chrono`** (RFC 3339 + 180-day staleness), **`zip`** (backup/restore), **`semver`** (update check) | `chrono =0.4.45`, `zip =9.5.1`, `semver =1.0.28` |

Spike evidence: `/tmp/a3-spike` — `cargo check --all-targets` clean, `cargo test` **5/5**
(`spike_tests`) + **2/2** (`mockall_proptest`), `cargo check --benches` clean (criterion).
Two spikes returned a **negative finding** (S-10 Linux compile, S-11 filename) — both are
documented below with their fallback; neither blocks the freeze.

---

## 1. S-4 — Serial transport: `serialport` 4.10.1 (primary), `serial2` 0.2.39 (fallback)

**Decision:** `serialport =4.10.1`, blocking I/O + `spawn_blocking` reader thread (per
SOFTWARE-PLAN SW-R5.2). `tokio-serial =5.5.0` is verified available as the async bridge
(depends on `serialport ^4` + `mio-serial ^5.0.3` + `tokio ^1.8`) but is **not** the first
choice — the plan's `spawn_blocking` design stands; tokio-serial is the escape hatch if the
engine team later wants the port as a tokio `AsyncRead/Write`.

**Verified today (crates.io API):** `serialport` **4.10.1** (updated 2026-09-08, 21.4M
downloads); `serial2` **0.2.39** (updated 2026-10-26, 24.9M downloads);
`tokio-serial` **5.5.0** / `mio-serial` **5.0.7** (both updated 2026-06-15).

**Why serialport first (verified in source, not marketing):**
- **COM9+ namespace:** `src/windows/com.rs:55-58` — `COMPort::open` prepends `\\.\` to the
  path unless it already starts with `\\`, so `COM10`…`COM256` open correctly.
- **DTR/RTS no-reset:** builder has `.dtr_on_open(false)` and `.preserve_dtr_on_open()`
  (`src/lib.rs:406-420`); the trait exposes DTR/RTS via `EscapeCommFunction`
  (`SETDTR`/`CLRDTR`/`SETRTS`/`CLRRTS`, `com.rs:293-303`). Opening the port must not reset
  the board — set `dtr_on_open(false)` and drive DTR/RTS explicitly.
- **Enumeration:** `serialport::available_ports()` returns `SerialPortInfo` with USB
  metadata (VID/PID/serial) — needed for COM auto-detect and dead-port skip (SW-R5.2).
  `serial2::available_ports()` returns only `Vec<PathBuf>` — no USB metadata.
- **Tokio bridge exists** for serialport (`tokio-serial`/`mio-serial`); `serial2` has no
  async integration at all.

**Maintainer risk — CONFIRMED still open:** the serialport README (main branch, read today)
still says verbatim: *"This project is looking for maintainers! Especially for Windows."*
Last release was 2026-09-08 (4.10.1) — maintained enough to ship, but the Windows plea is
real. That is why the fallback below is frozen now, not later.

**Fallback — `serial2 =0.2.39`:** `SerialPort::open(path, baud)`, `set_dtr(bool)` /
`set_rts(bool)` (verified `src/serial_port.rs:388/404`), `available_ports()`, concurrent
read/write from multiple threads, `windows-sys ^0.61` backend, MSRV 1.63. No tokio
integration — same `spawn_blocking` design works. **Swap trigger:** a Windows-only
serialport bug with no upstream fix inside one phase → swap behind the transport trait;
the trait boundary makes the swap local to `rsh-transport`. Do not swap for taste reasons.

**OPEN (Windows):** real COM-port open/read at 921600 baud, DTR-no-reset on a real ESP32
USB-serial adapter, and auto-detect ordering — Phase C gate on the owner PC / Windows CI.

---

## 2. S-5 — Crypto: `hmac` 0.13.0 + `sha2` 0.11.1; CRC hand-rolled; `ed25519-dalek` 3.0.0

**Decision:** `hmac =0.13.0`, `sha2 =0.11.1` (the plan's 0.13/0.11 claim is **correct** —
verified on crates.io today: hmac 0.13.0 updated 2026-03-29; sha2 0.11.1 updated
2026-10-12). `Hmac<Sha256>` over `nonce ‖ counter_LE ‖ cmd ‖ payload`, tag = first 16 bytes
(protocol v2.1 §4, frozen). `ed25519-dalek =3.0.0` (updated 2026-07-06; **MSRV 1.85**
confirmed in the crates.io index) — **verify-only**, no signing key in the app.
CRC16-CCITT stays **hand-rolled table-driven** (A1) — see below.

**Spike-proved API facts (the ones that will bite Phase C if missed):**
- `Hmac::<Sha256>::new_from_slice` needs `hmac::digest::KeyInit` in scope
  (`hmac` re-exports `digest`; `use hmac::digest::KeyInit as _;`). Without it the method
  does not resolve (spike hit this exact error).
- **Verify the 16-byte wire tag with `verify_truncated_left`, NEVER `verify_slice`.**
  `Mac::verify_slice` requires the tag to be exactly the MAC output length (32 B for
  SHA-256) and returns `Err` otherwise — the spike's first HMAC test **failed** exactly
  this way, and `digest 0.10.7` docs confirm: "`verify_slice` … Returns `Error` if `tag`
  is not valid or **not equal in length to MAC's output**", while `verify_truncated_left`
  is documented for truncated tags ("left side bytes, i.e. `tag[..n]`"). With
  `verify_truncated_left` the spike test passes for the right counter and rejects a
  wrong counter. This is the single most important S-5 finding.
- Pairing compiles: `hmac 0.13.0` depends on `digest ^0.11.2`, which is what `sha2 0.11.1`
  uses — the spike builds and runs them together.

**CRC — hand-rolled confirmed right.** The `crc` crate **3.4.0** (updated 2025-11-26) was
checked in source: it ships **no predefined CRC-16/CCITT-FALSE constant** (the only
predefined `u16` algorithm in the crate is `CRC_16_IBM_SDLC_NONREFLEX`, poly 0x1021 but
`refout: true, xorout: 0xffff` — the wrong variant). The algorithm must be spelled out by
hand anyway (`width 16, poly 0x1021, init 0xFFFF, refin false, refout false, xorout 0x0000,
check 0x29B1`), so the crate buys nothing over A1's table — and the `crc` crate would add
a dependency to `rsh-protocol`, which must stay pure/no-I/O. The spike proved the
parameters by feeding that hand-written `Algorithm` to the `crc` crate:
`"123456789" → 0x29B1` ✓ (D2 check value). A1's hand-rolled table stands; the golden
vectors remain the proof.

**Fallback:** if RustCrypto ever breaks the `hmac`/`sha2` pairing, `sha2` + a hand-rolled
HMAC (RFC 2104, ~30 lines, testable against RFC 4231 vectors) is the fallback. No
fallback needed for `ed25519-dalek` 3.0.0; 2.2.0 is the previous stable line if 3.x
regresses.

---

## 3. S-6 — Keystore (CRITICAL): `keyring` 4.2.0, DEFAULT features — the mock trap

**Decision:** `keyring =4.2.0`, **default features, no `default-features = false` anywhere**.
On Windows this enables the `v1` feature, which pulls in `windows-native-keyring-store`
(Credential Manager, DPAPI-backed). **Never use `keyring` 3.x.** D-R9 is closed by this
section.

**Verified today:**
- crates.io: `keyring` **4.2.0** (updated 2026-08-29, 40.0M downloads); latest 3.x is
  **3.6.3**. Index `rust_version` for 4.2.0 is **1.88.0** — fine on toolchain 1.100.
- **3.x trap — confirmed verbatim from docs.rs `keyring 3.6.3`:** 13 feature flags,
  **0 enabled by default**. The docs say: *"If you don't enable any credential stores
  that are supported on a given platform, the **mock** keystore will be the default on
  that platform"* and *"the notion of a default feature set has gone away: you must now
  specify explicitly which crate-supported keystores you want included (other than the
  `mock` keystore, which is always present)."* So `keyring 3.6.3` with no features
  silently stores secrets in memory and loses them on exit — exactly the trap in
  RESEARCH.md §2 / RSH-RUST-PLAN.md D-R9. A 3.x user would need
  `features = ["windows-native"]` to be safe.
- **4.x is fail-closed — confirmed in source** (`keyring-4.2.0/src/v1.rs:112-113`):
  `Entry::new` calls `windows_native_keyring_store::Store::new()?` on Windows (via the
  `v1` feature, which is in `default`). If the native store cannot be created,
  `Entry::new` returns `Err` — there is **no silent mock fallback** in 4.x; the mock
  exists only as `keyring_core::mock::MockCredentialStore`, used solely if a developer
  explicitly installs it (i.e. in tests). The spike proved the fail-closed direction on
  Linux: with no secret service present, `Entry::new` errors instead of silently
  accepting into a mock.
- **Blob limit 2560 B — confirmed:** `windows-native-keyring-store 1.1.0` imports
  `CRED_MAX_CREDENTIAL_BLOB_SIZE` (Windows constant = 5 × 512 = **2560 B**). Our secrets
  are tiny (32 B device key + 24 B Wi-Fi password), but note the store encodes secrets
  as **UTF-16LE** — a binary key must be base64/hex-encoded first (32 B → 44 base64
  chars → 88 B), still far under the limit. Enforce a `≤ 2560 B` assert in `rsh-security`
  before every write so a future larger blob fails loudly instead of truncating.

**The trap test (Phase C worker MUST write this — it is part of this decision):**
```rust
// tests/keystore_no_mock.rs — runs on Windows CI + owner PC, NOT on Linux CI.
#[test]
fn keyring_is_not_a_mock() {
    // 1. Write a canary in THIS process.
    let e = keyring::Entry::new("RSH", "__trap__").expect("native store must exist");
    e.set_password("trap-canary").unwrap();
    // 2. Read it back in a FRESH process (mock stores are per-process memory).
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--read-trap"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("trap-canary"),
        "secret did not survive a process boundary — MOCK STORE?");
    // 3. Cross-check with the OS: the entry must be visible in Credential Manager
    //    (e.g. `cmdkey /list:RSH` shows it). A mock passes (1) in-process but fails
    //    (2) and (3).
    // 4. Also assert Entry::store_status().is_ok() before any secret write in app code.
    e.delete_credential().unwrap();
}
```
Plus a unit-test rule: tests use `keyring_core::mock` **explicitly**; production code never
touches the mock, and `Entry::new` errors are surfaced to the user (pairing fails loudly),
never swallowed.

**Fallback:** if `keyring` 4.x ever regresses on Windows, the fallback is DPAPI-direct via
the `windows` crate (`CryptProtectData`/`CryptUnprotectData`) — more code, same OS store.
Do **not** fall back to `keyring` 3.x.

**OPEN (Windows):** the trap test itself, on Windows CI + the owner PC (cannot run here).

---

## 4. S-10 — Telemetry: `windows` 0.62.2, `cfg(windows)`-gated

**Decision:** `windows =0.62.2`, **`default-features = false`**, features
`Win32_Foundation`, `Win32_System_Memory`, `Win32_System_Diagnostics_ToolHelp`,
declared as a **Windows-only dependency** (`[target.'cfg(windows)'.dependencies]`) in
`rsh-telemetry`. Read-only mapping: `OpenFileMappingW` → `MapViewOfFile` (+ `UnmapViewOfFile`
/ `CloseHandle`); process list: `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` →
`Process32FirstW`/`Process32NextW`.

**Verified today:** crates.io `windows` **0.62.2** (updated 2025-10-06). API surface verified
in the generated source of `windows-0.62.2` itself:
`CreateFileMappingW` (`Memory/mod.rs:75`, returns `windows_core::Result<HANDLE>`),
`OpenFileMappingW` (`:392`), `MapViewOfFile` (`:329`, returns `MEMORY_MAPPED_VIEW_ADDRESS`),
`FILE_MAP_READ` (`:633`), `PAGE_READONLY` (`:1033`), `CreateToolhelp32Snapshot`
(`ToolHelp/mod.rs:2`), `Process32FirstW` (`:53`). All read-only; the game memory is never
written (C# behaviour preserved).

**⚠️ SPIKE FINDING — `windows` 0.62.2 does NOT compile on Linux.** `windows-future 0.3.2`
is a **non-optional** dependency of the `windows` crate, and it fails on Linux with 12
errors (`windows_core::imp::IMarshal` / `marshaler` not found; `windows_threading::submit`
not found — all `cfg(windows)`-gated in `windows-core`/`windows-threading 0.2.1`, whose
lib is `#![cfg(windows)]`). Reproduced with a minimal crate (`/tmp/win-min`: only
`windows =0.62.2`, `default-features = false`, two features — same 12 errors).
`default-features = false` does **not** fix it. Consequence, frozen here: any crate that
depends on `windows` must be `cfg(windows)`-gated, and **Linux CI must never build it**;
`rsh-telemetry` compiles a stub on Linux and the real readers only on Windows CI +
owner PC. This is standard practice, not a workaround — but it must be in the crate
layout from day one, or Linux CI goes red the moment telemetry lands.

**Fallback — `windows-sys =0.61.0`:** verified compiling cleanly on Linux
(`/tmp/winsys-min`, same features). Raw bindings (raw `HANDLE`s, no `Result` helpers) —
more `unsafe` code at every call site. Use only if the `windows` crate causes trouble on
Windows CI.

**Rejected — `shared_memory` 0.12.4:** last updated **2022-03-01** (4½ years stale),
5.8M downloads but effectively unmaintained, and it does not cover process enumeration
anyway. Not used.

**OPEN (Windows):** `windows` 0.62.2 compiling + the readers running on Windows CI;
live AC/ACC/iRacing shared-memory reads on the owner PC (Phase F).

---

## 5. S-11 — Logging: `tracing` 0.1.44 + `tracing-subscriber` 0.3.23 + custom writer

**Decision:** `tracing =0.1.44`, `tracing-subscriber =0.3.23`, `tracing-appender =0.2.5`
(versions verified on crates.io today: tracing 0.1.44 updated 2025-12-18;
tracing-subscriber 0.3.23 updated 2026-03-13; tracing-appender 0.2.5 updated 2026-04-17).

**⚠️ SPIKE FINDING — the C# filename `rsh-YYYYMMDD.log` (no dashes) is NOT producible by
`tracing-appender` 0.2.5.** Its DAILY date format is hardcoded in `src/rolling.rs:560-568`
(`[year]-[month]-[day]`) and the filename is always `{prefix}.{date}[.{suffix}]`
(`join_date`, `rolling.rs:629-656`) — there is no custom-date-format API. The spike
**proved it by running the appender**: the file created was `rsh.2026-11-14` (dashed,
dot-separated), and the spike test asserts both that a dashed file appears and that no
`rsh-*.log` file appears.

**Frozen resolution (keep the C# name):** `rsh-app-core` gets a small custom daily-rotation
`MakeWriter` (~60 lines, SW-R1.4 scope): rotate when the local date changes, open
`%AppData%\RSH\logs\rsh-YYYYMMDD.log` (no dashes), keep the non-blocking writer +
`WorkerGuard` pattern from `tracing-appender`, fall back to `%TEMP%\RSH\logs` when the
primary dir is unwritable (C# v2.0.4 lesson). The writer is unit-tested (fake clock/date
boundary → new file, old file untouched).
**Fallback:** if the custom writer fails review, accept `tracing-appender`'s
`rsh.YYYY-MM-DD.log` as the v1.0.0 name and document the rename in the USER-GUIDE
("see the log in %AppData%\RSH\logs" — the name changes, the location does not).
Either way the log dir, rotation (daily) and no-secrets rule are frozen; only the
filename mechanism is custom.

---

## 6. S-13 — Testing/bench: `criterion` 0.8.2, `proptest` 1.12.0, `cargo-fuzz` 0.13.2, `mockall` 0.15.0

**Decision:** `criterion =0.8.2` (dev-dependency, `harness = false` benches), `proptest =1.12.0`,
`cargo-fuzz =0.13.2` (binary, not a library dep), `mockall =0.15.0` (all dev-dependencies
except cargo-fuzz).

**Verified today:**
- `criterion` **0.8.2** (updated 2026-02-04, 339.5M downloads). Maintenance status,
  verified via GitHub: the repo moved to the **Criterion-rs organisation** (last commit on
  `bheisler/criterion.rs` 2025-11-29 redirects contributions there; repo pushed
  2026-04-23; 222 open issues) — i.e. **passively maintained by the original author, now
  org-maintained**, stable but slow. That is acceptable for a benchmark harness: 0.8.2
  compiles on rustc 1.100 (spike `cargo check --benches` clean) and its statistical
  model is what the benchmark gates (SW-R8.1) are written against.
- `criterion2` **3.0.5** (updated 2026-10-26, repo `Boshen/criterion2.rs`) is the
  **documented fallback**: actively released, same API family. Swap trigger: criterion
  0.8.x fails to build on a future toolchain, or a needed fix lands only in criterion2.
  Do not swap pre-emptively — the gates are written for criterion 0.8's output format.
- `proptest` **1.12.0** (updated 2026-10-10): spike `proptest!` case (CRC never panics
  on 0–128 random bytes) ran green.
- `cargo-fuzz` **0.13.2** (updated 2026-06-09): **installed and ran in this sandbox**
  (`cargo-fuzz 0.13.2`), and **nightly is available**: `rustup toolchain install nightly`
  succeeded — `rustc 1.102.0-nightly (5f4effd57 2026-11-13)`. The nightly requirement is
  therefore satisfiable in CI and locally. (No fuzz target was run in this spike — no
  target exists yet; that is SW-R2/SW-R8.2 work.)
- `mockall` **0.15.0** (updated 2026-06-28): spike `#[automock]` trait + expectation ran
  green.

---

## 7. Tauri version check — A2 pin confirmed current

**Verified today (crates.io):** `tauri` **2.12.5** is still the latest stable 2.x
(`max_stable_version 2.12.5`, updated 2026-11-06; it is also what `Cargo.lock` already
pins). **A2's pin needs no change.** Tauri 3 remains alpha — not used.

Latest stable 2.x plugin versions today (for the Phase E/H workers — **recorded only,
do not change any `Cargo.toml` here**):
| Plugin | Latest 2.x (verified) | A2 / workspace today |
|---|---|---|
| `tauri-plugin-single-instance` | **2.5.5** (2026-10-28) | 2.5.5 ✓ current |
| `tauri-plugin-autostart` | **2.7.0** (2026-10-14) | not yet pinned — use 2.7.0 |
| `tauri-plugin-updater` | **2.14.1** (2026-10-28) | not yet pinned — use 2.14.1 |
| `tauri-plugin-window-state` | **2.5.0** (2026-09-30) | not yet pinned — use 2.5.0 |
| `tauri-build` | (paired with tauri 2.12.5) | 2.7.4 ✓ |

---

## 8. Data dir — `dirs` 7.0.0 (A2 choice frozen)

**Decision:** keep A2's `dirs::data_dir() + "RSH"` → `%AppData%\RSH`. **Verified in source**
(`dirs-7.0.0/src/win.rs:10`): on Windows, `data_dir()` is
`known_folder_roaming_app_data()` — i.e. Roaming `%AppData%`, exactly the C# location.
Tauri's `app_data_dir()` resolves to the same Roaming path on Windows, but it needs a
Tauri `AppHandle`, which the headless library crates (`rsh-profiles`, `rsh-security`,
…) do not have. Using `dirs` keeps the data-dir logic testable without a Tauri app
and identical in every crate. `src-tauri` may use `app_data_dir()` for the same path —
add a test asserting both resolve to the same directory on Windows.

---

## 9. Date/time + misc — `chrono` 0.4.45, `zip` 9.5.1, `semver` 1.0.28

- **`chrono =0.4.45`** (features `["serde"]`) for `CalibratedAtUtc` (RFC 3339) + the
  180-day staleness check. Verified in the spike: `DateTime::parse_from_rfc3339`,
  `signed_duration_since(...).num_days() > 180` (2026-05-01 → 2026-11-14 = 197 days →
  stale ✓). **`time` 0.3.55 rejected:** it needs the `formatting` + `parsing` + `serde`
  features separately and its RFC 3339 path is less direct; `chrono` is the serde-JSON
  ecosystem default and its `serde` feature gives `DateTime<Utc>` (de)serialisation
  for free in profile JSON.
- **`zip =9.5.1`** (updated 2026-11-10) for backup/restore. Verified in the spike:
  `ZipWriter::new` + `start_file` + `ZipArchive` round-trip of a profile JSON ✓.
- **`semver =1.0.28`** (updated 2026-04-04) for the update check. Verified in the spike:
  `Version::parse("2.0.4") > Version::parse("2.0.3")` ✓.

---

## 10. Open items for the gate (not frozen here)

1. **Windows runtime:** keyring trap test (§3), serialport COM9+ at 921600 baud + DTR
   no-reset on real hardware (§1), `windows` 0.62.2 compiling on Windows CI (§4) —
   all **OPEN (Windows)**, Phase C / Windows CI gates.
2. **S-11 custom writer:** the ~60-line rotation writer is SW-R1.4 work, not A3 — its
   unit tests (date-boundary rotation, `%TEMP%` fallback) are the gate.
3. **`windows` crate on Linux CI:** `rsh-telemetry` must be `cfg(windows)`-gated from
   day one (§4) or Linux CI goes red.
4. **Tauri plugin pins** (§7) are recorded for the Phase E/H workers; the workspace
   `Cargo.toml`/`Cargo.lock` were **not** touched by A3.

## 11. Sources (all read 2026-11-14)

- crates.io API v1 (`/api/v1/crates/<name>`, `/versions`) + sparse index
  (`index.crates.io`) for every version, date and `rust_version` above.
- docs.rs pages: `keyring` 3.6.3 features + docs (mock-fallback text quoted in §3),
  `keyring` 4.2.0 (features: default `v1`), `serial2` 0.2.39, `windows` 0.62.2.
- Crate sources (downloaded `.crate` files, inspected in `/tmp/spike-src`):
  `keyring-4.2.0/src/v1.rs`, `windows-native-keyring-store-1.1.0`,
  `serialport-4.10.1/src/{lib,windows/com}.rs`, `serial2-0.2.39/src/serial_port.rs`,
  `tracing-appender-0.2.5/src/rolling.rs`, `dirs-7.0.0/src/win.rs`,
  `crc-3.4.0/src/crc16.rs`, `digest-0.10.7/src/mac.rs`,
  `windows-0.62.2/src/Windows/Win32/System/{Memory,Diagnostics/ToolHelp}/mod.rs`.
- GitHub: `serialport/serialport-rs` README (maintainer plea, main branch),
  `bheisler/criterion.rs` (org move, commits), `Boshen/criterion2.rs`.
- Spike runs: `/tmp/a3-spike` (5/5 + 2/2 tests, benches check clean), `/tmp/win-min`
  (windows 0.62.2 Linux failure repro), `/tmp/winsys-min` (windows-sys 0.61.0 Linux
  OK), `cargo-fuzz 0.13.2` install + `--version`, `rustup toolchain install nightly`
  → `rustc 1.102.0-nightly (5f4effd57 2026-11-13)`.
- Plans: `RSH-RUST-ARCHITECTURE.md` §2 (S-1–S-14) + §3.A (A3),
  `RSH-RUST-SOFTWARE-PLAN.md` §0 + §12 (R-1–R-12, U-1–U-7),
  `RSH-RUST-PLAN.md` D-R9, `RESEARCH.md` §2/§6, `SECURITY-INPUT.md` §0,
  `DATA-INPUT.md` §0/§8, `TESTING-INPUT.md` §0.1/§10.

## 12. Phase E gate decisions (2026-11-14)

Decisions made by the Phase E gate (coordinator, under the user's
delegation to use the plan defaults / best option). Phase E scope:
shell + navigation + 8 screens (E1–E6) + the two Tauri commands below.

1. **Only two commands wired at the Phase E gate.** `validate_pin_map`
   and `validate_firmware_image` are the only commands with a real
   library backend today, so they are the only ones implemented in
   `src-tauri` (thin wrappers over `rsh-app-core/src/commands.rs`,
   where the logic + tests live — `src-tauri` cannot compile in the
   Linux sandbox). The other 45+ commands stay `NOT_YET_IMPLEMENTED`
   in `frontend/src/lib/tauri.ts`; wiring stubs would be fake
   implementations, worse than an honest NOT_IMPLEMENTED. Phase F
   owns their backends.
2. **`validate_pin_map` mapping.** Flat `PinAssignment { function, pin }`
   → one config record each: steering/accelerator/brake/clutch/handbrake
   → `AXIS_ADC`; horn/buttons → `BUTTON`; led → `LED_STRIP`; validated by
   `rsh_protocol::validate_blob` (the same validator the board runs at
   COMMIT — single authority, no TS re-implementation). `shifter` /
   `encoders` / unknown functions are ERRORS, never silently skipped
   (they need multi-pin records the flat list cannot express — Phase F
   item: richer `PinAssignment` shape). Pin must be 0–39. Empty map is
   invalid (`validate_blob`: "at least one input module required").
3. **Anti-downgrade split.** `validate_firmware_image` verifies the
   Ed25519 signature + image format only, with current version
   `(0, 0, 0)` — it cannot reject a downgrade because it does not know
   the installed board version. The CALLER (Setup wizard) compares the
   returned `version` against `get_firmware_version` and refuses the
   upload on a downgrade; `upload_firmware` (Phase F) must re-validate
   with the same verifier before any upload. The frontend's check is a
   gate, not a trust decision. `signature_valid` is true only when the
   Ed25519 check actually ran and passed (or the image passed every
   check); truncated/unsigned/malformed images report `false`.
4. **`set_output_state` / `OutputState` moved into `tauri.ts`.** The
   E6 screen-local wrapper (`Output/outputState.ts`) is deleted;
   `tauri.ts` now owns `OutputState` (with optional `enabled` — absent
   until the backend reports it) and `setOutputState`. New wrappers
   added for Phase F commands: `list_backups`, `get_firewall_state`
   (both NOT_YET_IMPLEMENTED in `src-tauri`).
5. **CI `frontend` job: Playwright steps removed.** The A4 job ran
   `npx playwright install` + `npx playwright test`, but the frontend
   has no playwright config/tests — that step would fail on
   "No tests found" on the first green run. Removed with a comment;
   re-add only with landing playwright tests (E2E is a Phase F/H
   item). The job now runs: `npm ci`, `npm run lint`, `npm test`
   (vitest — includes the AppShell render test, the blank-window
   regression test), `npm run build` (`tsc -b && vite build`),
   `node scripts/merge-locales.mjs --check`, `npm audit --audit-level=high`.
6. **Locale merge is the single source of truth.** Screen strings live
   per-screen (`src/screens/<name>/strings.{en,ur,hi}.json`) and merge
   into `src/locales/*.json` via `scripts/merge-locales.mjs` (fails on
   collisions, key-set mismatches, single-brace `{var}`); re-run after
   any screen-string edit; CI runs `--check`. 417 keys per locale,
   identical key sets.
7. **No `matchMedia` test stub.** Adding one broke 3 Tuning tests: with
   `matchMedia` present, the uPlot dynamic import succeeds in jsdom and
   then fails on the missing canvas 2D context, hiding the SVG fallback
   path the tests cover. The lazy-import + SVG fallback design is the
   correct handling; a future static uPlot import must fail tests.
8. **Dark `--accent-deep` = `#C4B5FD`.** The UI plan says `#4B2FD0` but
   `RSHTheme.Dark.xaml` says `#C4B5FD`; the XAML value is used
   (`#4B2FD0` as text on the dark bg is near-unreadable). High Contrast
   hex values are CHOSEN (C# HC maps to Windows `SystemColors`, no hex
   exists) — recorded in `src/index.css`.
9. **Tuning has NO enable toggle.** UI plan §0 golden rule 2 + §P9.1:
   module toggles live ONLY in Settings; Tuning cards show a live
   On/Off/Unknown badge. Module on/off state is shared via the
   `modules` key of `get_settings`/`set_settings` (single source of
   truth with E4).

## 13. Phase H decision — 4-hour soak SKIPPED by user order (2026-11-14 14:54 PKT)

**Decision (user order, overrides master-plan D-R14):** the 4-hour (14,400 s)
long soak is SKIPPED — "Ye test skip kr dena, itna time ni hai" (time
constraint). D-R14 made the 4-hour soak a release blocker; the user
explicitly overrode that blocker. The 4-hour soak is therefore recorded as
**SKIPPED, not passed** — it must never be presented as passed in any gate
doc, release note, or report.

**What still runs (H2):** the 60 s quick soak and the 10 min full soak —
both fast, both give real stability signal (rate, loss, jitter). Their
results are reported in `docs/PERF-H2.md`.

**Consequence:** the Phase H gate doc and the v1.0.0 release criteria carry
this as an open, user-accepted item, verbatim: "4-hour soak SKIPPED by user
decision 2026-11-14 14:54 (time constraint); D-R14 release blocker
overridden by the user. 60 s + 10 min soak still run." The long-soak
evidence gap is a known, accepted limitation of this release — same as the
C# v2.0.4 history, where the 4-hour soak was also never completed.

## 14. Phase H gate decision — `soak_60s_no_growth` ACCEPTED with follow-up (coordinator, 2026-11-14 ~17:25 PKT)

**Test:** `tools/rsh-benchmarks/tests/soak.rs::soak_60s_no_growth` — 60 s
wall-clock paced, 12,000 ticks at 200 Hz, asserts heap growth == 0 via the
process-global counting allocator.

**Failure in the gate's full-suite runs:** heap grew by **594 net live
bytes, byte-identical in all 3 full-suite runs** (`cargo test --workspace
--exclude rsh-app`, two `--no-fail-fast` runs + the first run).

**Evidence it is a harness/context artefact, not a hot-path leak:**
- 594 bytes over 12,000 ticks = 0.05 bytes/tick. A real per-tick leak
  would be ≥ 12,000 bytes for 1 byte/tick — 20× larger than observed.
- 594 bytes is 0.6% of the pro-rated SLO budget (~100 KiB per 60 s from
  `soak_growth_mb_per_10min_max = 1`); the binding `0` assertion is far
  stricter than the SLO.
- `tests/alloc_gate.rs` passes 1/1 in every full run (10,000 ticks per
  section, 0 B/tick asserted in-process for the same pipeline / tracker /
  watchdog components).
- Standalone passes 3/3: quiet machine, artificial CPU load (3 spinning
  busy-loops), and an independent coordinator re-run (60.00 s) — CPU load
  alone does not reproduce it. A real leak would fail standalone too.
- The counting allocator is process-global and counts the libtest harness
  itself; its own docs warn the harness "must stay outside the measured
  window". The failure appears only when the full suite runs concurrently.

**Decision (coordinator, under the user's delegation; the exact 594-byte
source is NOT yet identified, so this is an accept-with-follow-up, not a
proven-environmental claim):** ACCEPTED as a gate item, NOT a blocker.
The release-grade growth evidence is the rig 10-min soak heap slope
(H2: 0.006, PASS — re-measured fresh in the Phase H gate rig run), not
this proxy.

**Documented follow-up (owned by the gate doc §7, not dropped):**
1. CI runs the soak proxy as a STANDALONE job (no parallel test
   binaries) — if it passes there, the full-suite failure is confirmed
   environmental.
2. Quiet the harness window: identify the 594-byte source (allocator
   instrumentation) and exclude harness allocations from the measured
   window (e.g. snapshot counts after a post-reset quiesce, or run the
   soak in a subprocess so the harness is outside the measured process).
   Until then the test stays as-is — no threshold weakened, no assertion
   tuned.

## 15. REVOCATION of §14 — `soak_60s_no_growth` accept is VOID (user order 2026-11-14 18:01 PKT)

**User order (2026-11-14 18:01 PKT): "Fix kro in ko. Mujy koi failure ya
bug ni chahiye."** This REVOKES the §14 decision in full. The
`soak_60s_no_growth` failure (594 bytes heap growth) is NO LONGER
accepted with a follow-up — it must be root-caused and FIXED (or proven
with evidence to be purely a harness artefact AND made to pass in the
full-suite run). A failing test in the gate run is not acceptable.

**Same order covers the rig failures:** soak loss 3.45% / long-soak
3.31% vs < 0.5% SLO, and ntp-ping e2e p99 21,768 µs vs 10,000 µs SLO —
all must be FIXED, none accepted. The Phase H gate verdict must be PASS
or CONDITIONAL PASS with ZERO failing tests and ZERO failing rig
scenarios. No failure or bug is carried into v1.0.0.

**Fix workers spawned by the coordinator (2026-11-14 ~18:02 PKT):**
FIX-A (soak proxy 594 bytes) and FIX-RIG (soak/long-soak loss +
ntp-ping + jitter p99 5001). After their fixes land: full gate re-run,
then the verdict, then the Phase H commit.

## 16. Deep-scan Round 2 decisions — Architecture + Packaging/CI fixes (2026-11-14, pre-release)

**Scope:** `.github/**`, `deny.toml`, `.gitignore`, `sbom/**`,
`docs/DECISIONS.md`, `docs/RELEASE-CHECKLIST.md`,
`docs/RELEASE-CHECKLIST-H4.md`, `docs/SYSTEM-INTEGRATION.md`,
`src-tauri/tauri.conf.json`, `src-tauri/src/main.rs` (doc comments only).
No code, frontend, or firmware touched; no commit, push, tag, or release
made by this worker. **CI PROOF PENDING (after push)** — every workflow
change below is YAML-validated and locally verified as far as the Linux
sandbox allows; the real CI proof comes from the first CI + release
dry-run after push.

### 16.1 H1 — rig-gate is fail-closed, `run-rig.sh` does not exist
The old job skipped while `run-rig.sh` was absent — but there never was
a `run-rig.sh`; the rig is the `rsh-rig` crate at `tools/rsh-rig`.
Decision: no wrapper script added; the predicate is now
`[ -d tools/rsh-rig ]` and the job runs
`cargo run -p rsh-rig -- --mode quick`. If the rig crate disappears the
job FAILS (missing-rig branch), it never skips again. Local check:
`cargo check -p rsh-rig` passes at HEAD 9690064 (verified in a clean
worktree); in the working tree the rig currently does not compile
because sibling deep-scan workers are mid-edit in `tools/rsh-rig/`
(`rss_slope_from` / use-after-move) — NOT this worker's files, NOT
caused by this change. Full `--mode quick` proof is deferred to the
gate re-run on a clean tree.

### 16.2 H2 — yanked check is fail-closed; `deny.toml` yanked = "deny"
Root cause: `cargo audit` exits 0 even when the yanked check never
completed (612× "couldn't check if the package is yanked" in the Phase
H gate; reproduced locally 2026-11-14, exit 0). Decision: the CI audit
job retries `cargo audit --deny warnings` up to 3 times (30 s apart)
and FAILS if the yanked verdict is still missing after the last
attempt; `deny.toml` sets `yanked = "deny"` (was `"warn"`) on the same
basis. Verified locally: `cargo deny check` passes with yanked =
"deny" (exit 0, all four checks ok) — cargo-deny resolves yanked
status from local advisory data and does not need the sandbox-blocked
registry fetch.

### 16.3 H3 — clippy scope is split Linux / Windows (deliberate deviation)
The audit asked for `cargo clippy --workspace --all-targets` on Linux.
That literal command is WRONG and was NOT implemented as written:
`tauri_build::build()` copies the `bundle.externalBin` sidecar at
build-script time and fails with `ResourcePathNotFound` when the staged
file for the current target triple is missing (proven from
tauri-build 2.7.4 `copy_binaries` + tauri-utils 2.10.2
`resource_from_path` sources). The sidecar is a Windows NativeAOT exe
and can only be staged on Windows, so a bare `--workspace` on Linux
would fail at the rsh-app build script. Decision: Linux clippy is
`cargo clippy --workspace --exclude rsh-app --all-targets` (covers all
12 other members, incl. the non-default rsh-benchmarks + rsh-rig) and
a NEW `clippy-windows` job runs `cargo clippy -p rsh-app
--all-targets` on windows-latest with the sidecar staged first.
Same root cause found in `test-linux` (`cargo test --workspace`
would also build rsh-app on Linux): fixed the same way
(`--exclude rsh-app`; rsh-app is tested on Windows). Both deviations
are documented in the `ci.yml` header so the next reader does not
"fix" them back.

### 16.4 H4 — release.yml is now a superset of the CI gates
A tag push does not trigger CI, so a tag could previously ship what CI
would have rejected. Decision: the release workflow now runs, before
the Tauri build, `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings` (Windows
runner, sidecar already staged), `cargo test --workspace`,
`cargo test --workspace --release` (the TESTING-INPUT §9.1 once-before-
release run), `cargo deny check`, and the full frontend gate
(`npm run lint`, `npm test`, `merge-locales --check`,
`npm audit --audit-level=high`, `npm run build`).

### 16.5 M1 — `cargo cyclonedx` does NOT produce the Rust SBOM (proven)
Local proof with cargo-cyclonedx 0.5.10 (installed pinned for the
check): `cargo cyclonedx --format json` writes one `<crate>.cdx.json`
PER member next to that member's Cargo.toml (14 files), never a root
`bom.json` — the old release step (`Copy-Item bom.json ...`) would
have failed fail-closed on a missing file. Its default spec is 1.3,
not 1.6; and even merged, its union tops out at 420 packages
(host target) / 589 (`--target all`) because dev-deps are excluded
by design — it can never reproduce the committed full-lockfile SBOM
(626 packages). Decision: the release workflow generates the Rust
SBOM with the new checked-in script
`.github/scripts/generate-rust-sbom.py` (`cargo metadata`, full
resolve, CycloneDX 1.6). Verified: the script reproduces the
committed file exactly (0 changed components) plus the missing
`rsh-rig` (see §16.11). Frontend SBOM split is deliberate: npm
10.9.4 caps at CycloneDX 1.5 — `--sbom-version 1.6` silently still
emits 1.5 (proven locally); 1.6 needs npm 11, which is not pinned.
The release requests 1.5 explicitly. Fresh `npm sbom` output matches
the committed frontend SBOM exactly (320 entries, same set), so no
regeneration was needed. Spec split, recorded: Rust 1.6, frontend
1.5, sidecar 1.6.

### 16.6 M2 — all `cargo install` tool installs pinned; nightly pinned
Unpinned installs float — a new tool release can break CI without any
repo change. Pinned (latest stable on crates.io, verified 2026-11-14):
cargo-deny 0.20.2, cargo-audit 0.23.0, cargo-fuzz 0.13.2,
cargo-cyclonedx 0.5.10 (install no longer used by any workflow —
kept as the proven version for ad-hoc use), tauri-cli 2.12.4
(3.0.0-alpha.6 exists — deliberately NOT pinned: a 3.0 alpha for a
2.x app). Fuzz nightly pinned to `nightly-2026-11-13` (channel
manifest verified to exist at static.rust-lang.org); the fuzz run
uses `cargo +nightly-2026-11-13`.

### 16.7 M6 — one shared sidecar script + a `gate` job
The fetch/verify/AOT-publish/stage block existed in three copies (CI
test-windows, CI tauri-build, release.yml) and had already drifted
once. Decision: single source of truth `tools/ci/stage-sidecar.ps1`
(pins + hashes as parameters, measured hashes emitted as step
outputs for M4); all three call sites + the new `clippy-windows`
job call it. New `gate` job aggregates all 12 CI jobs and fails
unless every one reports `success` — the single required check for
branch protection. (Note: the script lives under `tools/ci/`, outside
the original file-ownership list, because M6 names that exact path;
it is CI-only support code, not app/firmware code.)

### 16.8 M4 / M5 / M7 — SBOM verification, release enumeration, gitignore
- M4: the release verifies `sbom/sidecar.cdx.json` against the
  MEASURED staging hashes (step outputs, not re-typed constants) and
  the pinned version 2.1.0 — a drifted SBOM fails the run.
- M5: `gh release create` enumerates staged files explicitly
  (`Get-ChildItem release-staging -File`) instead of the
  `release-staging/*` glob.
- M7: `.gitignore` now covers `src-tauri/binaries/`,
  `release-staging/`, `bom.json`, and the per-crate
  `*.cdx.json` outputs (`crates/*`, `tools/*`, `src-tauri/*`).

### 16.9 L8 — merged-bin size check fails (documented exception only)
`Write-Warning` let a wrong-size firmware image sail into a release.
Decision: the check FAILS on mismatch. Exception: a firmware version
bump changes the size legitimately — then the expected size
(`$expectedMergedBinSize` in `release.yml`, 955,056 B for v4.0.1)
and the H4 §7 asset row are updated in the SAME change; the check
itself is never weakened alone.

### 16.10 L2 / L5 / L6 — stale updater docs, version checks
- L2: `docs/SYSTEM-INTEGRATION.md` §5 and `src-tauri/src/main.rs`
  `check_for_update` claimed the minisign keypair did not exist yet
  (`pubkey ""`, "flips `active: true`"). Reality: the keypair EXISTS
  (H4/G-4; pubkey in `tauri.conf.json`, private key in the
  `RSH_UPDATER_PRIVATE_KEY` secret), and there is NO `active` field
  to flip — `tauri-plugin-updater` 2.14.1 `Config` has no `active`/
  `dialog` field (Tauri v1 leftovers, silently ignored). The real
  gates are check-only app code + no `latest.json` published yet.
  OPEN GAP (deferred to the Windows/human gate): the
  bad-signature-rejects-update test was never written — it needs a
  Windows run asserting the updater refuses a tampered signature.
  Until it exists, the updater stays check-only.
- L5: `tauri.conf.json` cannot derive the window title from the
  version (static JSON), so the release version check now also
  asserts the title equals `RSH — Real Sim Hub v<version>`.
- L6: the release version check now also asserts
  `frontend/package.json` version matches the tag.

### 16.11 L7 — package recount: 626, not 625 (L1 fixed alongside)
Recount (2026-11-14): `Cargo.lock` has 626 `[[package]]` entries
(`cargo metadata` agrees: 626 packages); the committed Rust SBOM
listed 625 — the one missing package was `rsh-rig` (the test rig,
not shipped). Decision: the SBOM was regenerated with the §16.5
script to 626 components (0 other changes); `docs/RELEASE-CHECKLIST.md`
and `docs/RELEASE-CHECKLIST-H4.md` now say 626. NOT fixed here:
`THIRD-PARTY-LICENSES.md` still says 625 — it is outside this
worker's file ownership; the coordinator must update it to 626 in
the same change (same for `docs/PHASE-H-GATE.md`, which also says
625). Frontend count is 320 entries (318 unique name@version —
`magic-string@0.30.21` legitimately appears 3× at different install
paths); that number was already correct.
- L1 (same edit round): `deny.toml` said "All five are transitive"
  and "Review date for all five" for the advisory ignores, but the
  list has FOUR entries — corrected to "four" (the other "five" in
  the file, the five permissive license extras, is correct and kept).

### 16.12 L3 / L4 — CI header rewritten; interim icons noted
- L3: the `ci.yml` skip-map header described Phase A (jobs skipping
  while phases had not landed). Rewritten to Phase H reality: every
  prerequisite has landed, NO job is expected to skip anymore, the
  remaining check steps are tripwires, and the Linux/Windows split
  (§16.3) is documented in the header.
- L4 (no action): the icons in `src-tauri/icons/` are H4-generated
  interim assets (PIL, violet `#6C4CF1` rounded square + white `R`) —
  a brand mark, not a final logo. No action taken; if the UI plan
  ever ships a real logo, replace the files and rebuild (H4 §2).

### 16.13 Deferred to the Windows / human gate (not fixed here)
- First CI run + first release dry-run after push (the actual proof
  for H1–H4/M2/M6 — see "CI PROOF PENDING" above).
- Bad-signature-rejects-update test (L2 gap, §16.10).
- 4-hour soak: SKIPPED by user order (§13) — unchanged, never
  presented as passed.
- `THIRD-PARTY-LICENSES.md` 625 → 626 (outside this worker's
  ownership, §16.11).
- Real updater end-to-end (manifest fetch, signature verify, install)
  on the owner PC; `cargo tauri build` producing NSIS + MSI on
  windows-latest (already listed as Windows-only in H4 §8).

## 17. Deep-scan gate decisions (2026-11-14, gate worker — see `docs/DEEP-SCAN-GATE.md`)

Gate verdict: **CONDITIONAL PASS** — zero failing tests and zero failing rig scenarios locally (Rust 901/0, frontend 241/241, rig full 27/27 seed 42 + multi-seed 54/54, firmware host tests 4444/0, fuzz 500,000 runs 0 crashes). The conditions are exactly the ones every phase carried: first green CI + release dry-run after push (post-push proof), and the Windows/human gates (§17.6). Nothing local is left unproven.

### 17.1 4-hour soak: SKIPPED by user decision — never as passed (§13 unchanged)
The 4-hour soak stays SKIPPED by the user order of 2026-11-14 14:54 PKT. It is recorded as SKIPPED in the release notes, never as passed. The 10-min rig soak (long-soak, 600 s) PASSED this gate (loss 0%, coalesced 0%, heap slope 0.001 MB/10 min) and the 60 s soak proxy in `cargo test` PASSED — those are the measurements that exist; they do not stand in for the 4-hour run.

### 17.2 CI proof is a POST-PUSH condition (as in all previous phases)
The gate verdict rests on LOCAL evidence only. First green CI run + first release dry-run after push remain the proof for the CI/release-workflow fixes (H1–H4, M2, M6, L8). The gate does not claim them proven. `cargo audit`'s yanked check and `npm audit` could not complete in this sandbox (registry timeouts / 403 policy_denied) — carried to CI the same way (Phase H F-4). `cargo deny check` with yanked = "deny" PASSED locally and is the fail-closed authority for the yanked verdict (§16.2).

### 17.3 Dialog plugin compile is a Windows gate (W-1)
Round 2 registered `tauri-plugin-dialog` (Cargo.toml 2.8.4, `main.rs` init, capability, README). The Linux sandbox cannot compile the Windows target (`lib.exe` missing — `cargo check -p rsh-app --target x86_64-pc-windows-msvc` fails in `cc-rs` exactly there, as expected). The dialog compile is Windows gate W-1: proven by the first Windows CI build / `cargo tauri build`, not by this gate.

### 17.4 Merged-bin size moved 955,056 → 955,984 B (same-version code change)
Round 2 firmware changes (SEC-R2-03 AP auto-lock, SEC-R2-04 session rate-limit, M-5 FACTORY_RESET handler) grew the app by 928 B. The gate clean-rebuilt twice (deterministic SHA-256 `1a132de9fbd4363e3d674f751b054b1a522d8c45f2543823109515e37b6f8368`) and updated `$expectedMergedBinSize` in `release.yml` (955984) plus every live size note (RELEASE-CHECKLIST-H4, RELEASE-CHECKLIST, FLASH-GUIDE, REFLASH-GUIDE, README, FIRMWARE-COMPAT §7) in the same change, per the L8 rule (§16.9). Historical run records (PHASE-H-GATE, FIRMWARE-COMPAT §5) keep the old size — they describe that run. Signed bin is now 890,516 B (app 890,448 + 68 B trailer).

### 17.5 SBOM recount: 629, not 626 (dialog plugin deps)
`Cargo.lock` has 629 `[[package]]` entries; the committed Rust SBOM was regenerated with the §16.5 script to 629 components (added exactly `rfd` 0.17.2, `tauri-plugin-dialog` 2.8.4, `tauri-plugin-fs` 2.7.1; 0 removed). `THIRD-PARTY-LICENSES.md` now says 629 with a corrected license table (rows sum to 629: MIT 148 incl. all 13 RSH crates, `Apache-2.0 OR MIT` 65; the old table's rows summed to 623 — `r-efi`'s 2 packages lived only in §1.3, now listed in the 2-each row). Frontend stays 320, sidecar 2.

### 17.6 Windows / human gates carried (not proven by this gate)
- W-1: dialog plugin compile on Windows (`cargo check -p rsh-app --target x86_64-pc-windows-msvc` / first `cargo tauri build`).
- W-2: foreground-detector E2E (property test exists; live Windows E2E outstanding).
- A7: real-board run, once, at final production release (owner PC).
- Clean-PC install Win10 22H2 + Win11 (NSIS + MSI produced by the Windows build; config dry-checked only).
- Game matrix 0/16 — no pair verified without evidence.
- Real-Wi-Fi ping (rig NTP-ping is loopback-equivalent; p99 6,438 µs in-process).
- C# sidecar compile against the real HIDMaestro DLL (DLL absent in sandbox; ZIP/DLL SHA-256 pins verified against the SBOM, staging script read-through PASS).
- src-tauri Windows compile (tauri_build sidecar copy; `--workspace` on Linux is wrong per H3 — `--exclude rsh-app` used).
- HAT_NEUTRAL_RAW 0x08 verify (board-side).
- ACC map-name live confirm.
- Security M-4: flash encryption, board-side (open).
- vJoy DLL SHA-256 pin unmeasured (`PINNED_DLL_SHA256_HEX` still deliberately `None`).

### 17.7 Threshold re-measure, not a baseline change
The benchmark gate first failed on the regression gate only: `pin_validate_blob_factory` 9,201.6 ns vs the 1.25x limit 8,232.1 ns (plan cap 9,879 ns still passed). The script's own instruction is "re-measure and explain". Re-measure on the quiet host: 7,567.9 ns — inside the 1.25x limit and inside the documented ~18% run-to-run spread for pin_validate. No product code touches pin validation (`crates/rsh-protocol/src/pin.rs` untouched by any deep-scan change), so this was host noise, not a regression. No baseline or threshold was raised. All thresholds PASS after the re-measure.

### 17.8 H-2 196 Hz limitation is now historical for coalescing
The old latest-wins drain coalesced ~3–14% of frames at exactly 200 Hz, which is why the soak emits at 196 Hz. After the Round 2 multi-frame drain, `emit-200hz` at true 200 Hz recorded **coalesced 0%** (6,047 valid packets, 6,048 ticks, loss 0%, rate 201.35/s) on all three seeds (42, 7, 1337). The 196 Hz soak emit is kept as the documented soak default by design, not because the drain needs it. Stale "latest-wins" wording in the rig (harness/scenarios/main/wheel) was corrected to the multi-frame drain in this gate.

### 17.9 Merged-bin pin moved 955984 → 956032 B (host-toolchain variance, NOT a code change)
Dry run #9 (2026-11-15, run 43670136393, head 49f011a9) failed ONLY at the fail-closed merged-bin size check: Windows runner built 956,032 B (0xe9680, app 890,496 B, image SHA-256 d6d15d3d…) vs the pinned 955,984 B. Root cause proven, not assumed: (a) zero firmware/ commits since af4cb53; (b) local clean rebuild reproduces 955,984 B byte-identical (SHA 1a132de9…6f8368) AND the CI Linux firmware job on the same head also built 0xe9650 = 955,984 B; (c) both hosts used PIO 6.2.0 + pinned espressif32@6.9.0 / toolchain-xtensa-esp32 8.4.0+2021r2-patch5 / framework-arduinoespressif32 3.20017.241212 / esptool 4.5.1 — so the +48 B is Windows-vs-Linux host-toolchain codegen, same source. The release workflow runs ONLY on windows-latest, so the Windows-runner value is the correct pin; the check stays exact/fail-closed (no threshold, no range). Pin + every live size note (RELEASE-CHECKLIST-H4, RELEASE-CHECKLIST, FLASH-GUIDE, REFLASH-GUIDE, README, FIRMWARE-COMPAT §7) updated in the same change per the L8 rule. Dry run #10 must reproduce exactly 956,032 B on a fresh runner (2nd independent measurement); if it differs, STOP and investigate non-determinism instead of moving the pin again.

### 17.10 Dry run #10: updater secret format + SBOM swallow + debug-test removal
Dry run #10 (run 43673570156, head 48b7aa0) failed ONLY at step 25 `cargo tauri build`: `failed to decode secret key: Invalid symbol 32, offset 9`. Root cause: `RSH_UPDATER_PRIVATE_KEY` held the whole 2-line `.key` file; Tauri decodes the secret as bare base64 and chokes on the space after "untrusted". Fix: secret now holds ONLY line 2 (212-char base64, canonical 158-byte minisign-crate blob, kdf none — verified structurally: starts "Ed", keynum matches the .pub file; H4 had already verified crate-level parse + sign→verify round-trip). `set-updater-secret.py` updated to store line 2 only + validate format before PUT (value never printed); signing-keys/README.md corrected the same way. Everything BEFORE step 25 passed in #10: debug + --release tests, frontend gates, PIO build + "Firmware signed + verified" (merged 956,032 B on a 2nd fresh runner — the #9 pin is proven), sidecar SBOM verify, NSIS (10.96 MiB) + MSI (15.99 MiB) built. Second issue, same run: `generate-rust-sbom.py` crashed (`cargo metadata` stdout decoded as cp1252 on Windows, byte 0x81) and the pwsh step swallowed it — "SBOMs regenerated" printed with no sbom-rust.cdx.json written. Fix: script captures bytes + decodes UTF-8 explicitly (fail-closed, no errors="replace" on data) and the workflow step now checks `$LASTEXITCODE` after every native call + verifies all three SBOMs exist and parse with components. (The missing SBOM was never caught in #10 because the run died at tauri build before the stage-verify step — the fail-fast check closes that hole.) Third change, user order 2026-11-15 08:51: the redundant debug `cargo test --workspace` step is removed from release.yml — CI proved the debug suite green on this tree (run 43673557870, head 48b7aa0, 13/13; run 43669130447, head 49f011a9, 13/13). The --release run, frontend gates, PIO build + firmware verify, SBOM, tauri build and stage/verify all stay.

### 17.11 Real release attempt 1+2: updater key MUST be password-encrypted — new keypair (2026-11-15)
Real release run 43678000277 (tag v1.0.0, head ef72d973) failed TWICE at step 24 `cargo tauri build`, both times only at updater signing. Attempt 1: `failed to convert base64 to utf8: invalid utf-8 sequence of 1 bytes from index 55` — the line-2-only secret (§17.10 fix) decoded to the 158-byte binary blob, not UTF-8. Attempt 2, after the secret was corrected to base64(whole 2-line .key file) — decode + UTF-8 now PASS, but: `incorrect updater private key password: Key is not encrypted`. Root cause proven from tauri-cli 2.12.4 + minisign 0.9.1 source (local cargo registry): `secret_key()` ALWAYS calls `sk_box.into_secret_key(password)` — the ENCRYPTED path; `from_box_(unencrypted_key=false)` returns "Key is not encrypted" for `kdf_alg == KDF_NONE`; there is NO unencrypted-key path in the bundler. And `bundle.rs` always passes `Some(password)` in CI (`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` unset → `Some("")`). Consequence: the H4 keypair (minisign-crate `generate_unencrypted_keypair`, kdf none) could NEVER work with `cargo tauri build` — no secret-value change can fix it. Fix (standard Tauri flow, no shortcut): NEW keypair generated with `cargo tauri signer generate` + 32-char random password (from /dev/urandom); password in `signing-keys/updater-password.txt` (0600, outside repo) and new GitHub secret `RSH_UPDATER_PRIVATE_KEY_PASSWORD`; `RSH_UPDATER_PRIVATE_KEY` = contents of the CLI-generated `.key` file as-is (348-char base64 of the 2-line file — the CLI writes base64, so re-encoding would double-encode); `release.yml` `cargo tauri build` step now sets `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; `tauri.conf.json` `plugins.updater.pubkey` = base64 of the new `.pub` file (152 chars — the bundler's `pub_key()` also base64-decodes the config pubkey; the old bare 56-char line would have failed right after the secret step). New keypair: pubkey line `RWTeanojSpk/AqwtLj5+ShAZPbWWzRmQugDCs9fJUjVrtNkCJ2+llrE2`, keynum `023F994A237A6ADE`, pubkey hex `ac2d2e3e7e4a10193db596cd1990ba00c2b3d7c952356bb4d902276fa596b136`, fingerprint `ecb5a3106e30468090ea39c90f69498e2ae646f13b8cace256bcfea6267fc1f1`, kdf `Sc`, chk `B2`. Local proof BEFORE push (release-tools/ms-proof, minisign 0.9.1 + minisign-verify 0.3.0): (A) bundler secret path — base64(.key file) → UTF-8 → `from_string` → `into_secret_key(Some(password))` decrypts; (B) bundler pubkey path — base64(.pub file) → UTF-8 → `from_string` → `into_public_key`; (C1) `cargo tauri signer sign` output (.sig 400 B) verifies with the new pubkey; (C2) bundler-path signature verifies; (C3) wrong password rejected. Retired keys kept in signing-keys/ as `*.unencrypted-RETIRED-2026-11-15` (H4 keypair, never shipped) and `*.empty-pw-RETIRED-2026-11-15` (empty-password encrypted pair generated 06:07 and retired minutes later, never used anywhere — superseded by the password-protected pair per the parent order); shred them once v1.0.0 is green. Rotation is safe: v1.0.0 was never published, so no installed app holds the old pubkey. `set-updater-secret.py` rewritten for the CLI file format (contents as-is + password secret). tauri-cli 2.12.4 was compiled locally to do this (install took 17m 47s; first attempt failed — /tmp is a 512M tmpfs at 100%, linker SIGBUS "No space left on device" — fixed by CARGO_TARGET_DIR under ~/workspace; /tmp also wiped /tmp/ms-test, so the proof program now lives persistently at release-tools/ms-proof/). Tag v1.0.0 moves to the new commit; the tag push triggers the real release workflow again (no dry run, user order 2026-11-15 09:39).

### 17.12 Real release run 43687032820: SIGNING PROVEN on the runner — stage step had the wrong bundle path
Run 43687032820 (tag v1.0.0 moved to de7e3902, the §17.11 keypair-rotation commit) got PAST `cargo tauri build` for the first time: the bundler printed "Finished 2 updater signatures" (`RSH_1.0.0_x64-setup.exe.sig` + `RSH_1.0.0_x64_en-US.msi.sig`) — the password-protected keypair + both secrets + `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` env work end-to-end on the real Windows runner. CI on the same head (run 43687008080) also completed success. The run then failed ONLY at step 25 "Stage release assets": `Expected exactly one NSIS setup exe in src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis`. Root cause: the stage step assumed the bundle dir is under `src-tauri/target/`, but src-tauri is a workspace member, so Cargo writes the bundle to the WORKSPACE-ROOT target dir — the log shows `D:\a\RSH\RSH\target\x86_64-pc-windows-msvc\release\bundle\nsis\RSH_1.0.0_x64-setup.exe` (both exe + msi + both .sig files existed there). This path was never exercised before: every earlier run died at signing (step 24), so the stage step never ran. Fix: `$bundleDir = "target/x86_64-pc-windows-msvc/release/bundle"` (one line + comment). No other step references `src-tauri/target`.
