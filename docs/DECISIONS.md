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
