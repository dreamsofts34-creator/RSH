# RSH — HIDMaestro Interop Spike G1 (SW-R6.2)

**Status: SPIKE REPORT — 2026-11-14. DECISION: GO — path (b) C# NativeAOT sidecar.**
**Pinned version: HIDMaestro v2.1.0** (latest release as of 2026-11-14; GitHub
`hifihedgehog/HIDMaestro`, released 2026-10-27). No production code was written.
Prototypes live in `~/workspace/projects/rsh/rust-recreation/spike/g1-hidmaestro/`
(source only — no DLLs, no binaries). This file is the spike's only repo file.

**Headline caveat (read first):** the HIDMaestro path needs an **elevated helper
process for the whole session** — not just a one-time install. The input shared
section's DACL grants Everyone only `GENERIC_READ`; only an admin/SYSTEM/
LocalService process can write input frames. So RSH with the HIDMaestro backend
will show a **UAC prompt on every launch** (or RSH itself must run elevated).
vJoy needs no prompt at runtime (one-time admin install only). This is a real UX
cost of the HIDMaestro primary — see §6. It does not change the GO decision
(the user already chose HIDMaestro as primary), but it must be visible in the
installer/UX design (G2 + H4) and in the release notes.

## 1. Decision

**GO — path (b): C# NativeAOT sidecar process (`rsh-hm-sidecar.exe`), speaking
a JSON-lines protocol over stdin/stdout to the Rust app.**

- The sidecar owns the whole HIDMaestro lifecycle: `HMContext` →
  `InstallDriver()` → custom no-brand wheel profile → `CreateController` →
  `SubmitState`/`SubmitRawReport` → `Dispose`. Rust never touches the SDK.
- Transport: stdin/stdout JSON-lines (one command per line, one reply per
  line). No named pipe in v1.0 — pipes add ACL/namespace work for zero
  measured benefit at our frame rate (see §3).
- Profile: **custom no-brand wheel profile `rsh-wheel-custom`**, built at
  runtime in the sidecar with `HMProfileBuilder` + `HidDescriptorBuilder`
  (proven to build — §4). NOT a catalog brand profile, NOT
  `generic-dinput-gamepad`.
- Fallback stays: vJoy FFI backend (SW-R6.4). Any sidecar failure (no admin,
  driver install fails, sidecar crash) → clean fallback to vJoy + clear error,
  never a crash, never a silent dead wheel.
- Build: NativeAOT single exe, **built on Windows** (cross-OS NativeAOT is
  unsupported — measured §3). Fallback publish (self-contained win-x64,
  buildable on Linux) is 95 MB and is only a stopgap.

**Rejected: path (a) direct shared-memory from Rust — NO-GO as primary.**
The wire protocol is fully documented byte-level (see §2), so a Rust writer is
*possible in principle* — but it is the wrong call for v1.0:

1. **Install has no native equivalent.** `InstallDriver()` stages embedded
   driver files, SHA-256-checks them, creates + installs a self-signed
   code-signing certificate, signs the binaries, and registers them with
   `pnputil`. Reimplementing cert creation/signing/pnputil orchestration in
   Rust (via `windows-rs` SetupAPI/Crypt32) is a large, fragile lift with no
   documented stable contract — and a mistake here bricks the install or
   leaves a half-trusted cert behind.
2. **Device-create is SDK-internal orchestration, not a stable API.**
   `CreateController` runs per-archetype `SetupController`: registry writes
   (`HKLM\SOFTWARE\HIDMaestro\Controller<N>` + `Identity\<token>`) →
   `pnputil /add-driver` if needed → SwDevice/SetupAPI devnode creation →
   PnP wait → XInput slot-claim wait → driver protocol handshake (version
   section, protocol 4/5). None of that sequence is a versioned contract;
   reimplementing it means tracking every SDK release line-by-line.
3. **The DACL blocks a non-elevated Rust writer anyway.** Input section SDDL
   is `O:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;LS)(A;;GR;;;WD)` — Everyone
   gets `GENERIC_READ` only. A non-elevated Rust process literally cannot
   write frames; only the elevated creator can. So path (a) does not even
   remove the elevation requirement — it just moves the same requirement onto
   the Rust process (RSH would have to run elevated) or onto a
   handle-duplication dance with the sidecar.
4. **Protocol churn.** v2.0.0 added the version section (protocol 4/5); v2.1.0
   added the signal section and rewrote frame delivery. A native
   reimplementation must track each release exactly, with no LTS/stable
   branch to pin to. The SDK absorbs that churn for us on path (b).
5. **No latency need.** IPC costs ~70 µs p50 / ~276 µs p99 at 200 Hz (§3)
   against a 10 ms E2E p99 SLO — ~35× headroom. Saving ~50 µs by writing
   shared memory directly buys nothing measurable.

A **hybrid** (sidecar creates/owns the device, Rust writes frames directly into
the shared section via a duplicated handle) remains technically possible and is
recorded as a future optimisation — explicitly NOT for v1.0: it saves
microseconds nobody can feel and adds handle-lifetime + DACL + version-skew
risk. If a future profile ever shows IPC as the bottleneck (it will not at
200 Hz), revisit then.

## 2. What the driver exposes (path (a) research — docs + measured API)

Sources: `hidmaestro.org/docs/reference/shared-memory-protocol/` (724 lines,
read in full), `/docs/reference/umdf2-driver-internals/` (1,088 lines),
`/docs/sdk/sdk-reference/` (1,809 lines, read in part), plus live reflection
against the real v2.1.0 DLL (see §3).

**Driver stack:** `HIDMaestro.dll` is a UMDF2 lower-filter driver under
Microsoft's inbox `mshidumdf.sys` (+ `WUDFRd.sys` reflector). Kernel runs only
Microsoft-signed inbox components — this is the HVCI advantage over vJoy's
third-party `vjoy.sys`. No kernel driver for RSH to sign.

**Shared-memory sections** (per transport index `<N>`, names from
`HMController.SectionNames` / `TransportIndex` — never hardcode the slot):

| Section | Name | Size | Direction |
|---|---|---|---|
| Input | `Global\HIDMaestroInput<N>` | 362 B | SDK → driver: seqlock'd HID input frames (`SeqNo` + `DataSize` + `Data[256]` + `GipData[14]`) |
| Output | `Global\HIDMaestroOutput<N>` | ~16.5 KB (64-slot ring) | driver/companion → SDK: rumble/haptics/FFB capture |
| PID State | `Global\HIDMaestroPidState<N>` | 40 B (36 used) | both: HID PID 1.0 state mirror, EBI bitmap |
| Input Ack | `Global\HIDMaestroInputAck<N>` | 688 B | driver → SDK: last delivered seqno |
| Version | `Global\HIDMaestroVersion<N>` | 64 B | both: SDK/driver versions + protocols + identity tag (FNV-1a of token) |
| Signal (v2.1.0) | `Global\HIDMaestroSignal<N>` | 256 B | both: worker-wait counts + frame handoff |

Plus named events (`InputEvent`, `InputAckEvent`, `Remove/RemovedEvent`,
`OutputEvent`, `StopEvent`) and the owner pipe `\\.\pipe\HIDMaestroOwner<N>`,
held by the SDK for the controller's life (driver sends release frames when
it closes; a program that exits without disposing has its devices removed,
≤10 s sweep).

**SDK public surface (measured by reflection on the real DLL):**
`HMContext` (21 public methods), `HMController` (32), `HMProfile` (60),
`HMProfileBuilder` (30), `HidDescriptorBuilder` (19). Key calls:
`HMContext()` (non-blocking; prewarms catalog parse + driver staging +
GameInput, 200–500 ms hidden in think time), `IsDriverInstalled` (no admin
needed), `InstallDriver()` (admin; `AlreadyCurrent` fast path when this
build's driver is installed — even non-elevated), `LoadDefaultProfiles()`,
`GetProfile(id)`, `CreateController(profile, identityKey)` (admin; returns
only once the device is fully bound), `HMController.SubmitState(in
HMGamepadState)` / `SubmitRawReport(ReadOnlySpan<byte>)`,
`PublishPidPool/BlockLoad/State` (FFB — v1.0 out of scope), `DriverVersion`,
`TransportIndex`, `SectionNames`, `OnSubmitLatencyMicros` (built-in
per-submit latency hook — G2 should wire this into diagnostics),
`RemoveAllVirtualControllers()` (admin; uninstall path).

**What a native Rust writer would have to do:** create the five sections +
events with the exact SDDL above, write the version section (layout v1,
SDK protocol 4, identity tag), write `HKLM\SOFTWARE\HIDMaestro\Controller<N>`
(descriptor, VID/PID, strings, `InputReportByteLength`, `InputReportId`,
`FeatureAnswers` table — compiled from the profile by SDK-internal logic),
write `HKLM\SOFTWARE\HIDMaestro\Identity\<token>`, create the devnode
(SwDevice/SetupAPI), wait for PnP bind + driver protocol handshake, then
write frames with the seqlock protocol (odd `SeqNo` → barrier → payload →
even `SeqNo` → `SetEvent` only while the worker waits, per the v2.1.0 signal
section). Every step is admin-gated and version-sensitive. Doable; not sane
to own for v1.0.

## 3. Measurements

All numbers below were measured in this spike (2026-11-14, Linux sandbox)
unless marked "vendor claim" or "proxy". Sandbox-blocked items are labelled.

### 3.1 Pinned artifacts — independently re-verified

| Artifact | Measured | Pinned (C# constants, 2026-11-13) | Verdict |
|---|---|---|---|
| Release ZIP `HIDMaestro-v2.1.0.zip` (55,148,357 B), fresh download from `github.com/hifihedgehog/HIDMaestro/releases/download/v2.1.0/` | SHA-256 `7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4` | `7fe349f1…97b4` | **MATCH** |
| `HIDMaestro.Core.dll` (18,163,712 B), extracted from the fresh ZIP | SHA-256 `f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5`, byte-identical (`cmp`) to the vendored copy in the C# backup | `f13e3259…b4d5` | **MATCH** |

**Pinned version: v2.1.0.** ZIP SHA-256
`7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4`;
DLL SHA-256
`f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5`.
License MIT (`LICENSE` + `THIRD-PARTY-NOTICES.txt` in the release ZIP).
Latest release as of 2026-11-14 (GitHub API): **v2.1.0, 2026-10-27** — no
newer release exists. 30 releases total; cadence is fast (v2.0.0 → v2.1.0 in
6 days, 2026-10-21 → 2026-10-27). Pin + re-verify on every upgrade.

### 3.2 Sidecar prototype (path b) — built and measured

Prototype source: `rust-recreation/spike/g1-hidmaestro/sidecar/` (C#,
`net10.0-windows`, references the v2.1.0 DLL; JSON-lines stdin/stdout;
commands `ping / version / install-driver / create / submit / neutral /
dispose / exit`; `create` supports a catalog id or the custom
`rsh-wheel-custom` profile). .NET 10 SDK 10.0.404, `~/.dotnet`.

| Check | Result |
|---|---|
| `dotnet build -r win-x64` (cross-compile on Linux) | **PASS**, 3.6 s, 0 errors, 0 warnings |
| `dotnet publish -r win-x64 --self-contained` (Linux → Windows) | **PASS**, 15.2 s, publish dir **95 MB** (runtime + 18 MB DLL); output is a real PE32+ x86-64 Windows exe (`file` confirmed). Fallback only — too big to ship happily. |
| `dotnet publish -r win-x64 -p:PublishAot=true` (Linux) | **FAIL — sandbox/toolchain block, measured:** `error: Cross-OS native compilation is not supported.` (ILCompiler 10.0.15). NativeAOT **must be built on Windows** (Windows CI runner or owner PC). |
| NativeAOT + `System.Text.Json` reflection | **IL3050 / IL2026 warnings** (trims/AOT-unsafe): G2 must use source-generated JSON (`JsonSerializerContext`) or a hand-rolled binary framing — reflection JSON will break under AOT. Prototype noted; G2 fixes. |
| NativeAOT size (proxy, not measured) | Vendor's own `HIDMaestroUninstall.exe` in the same release ZIP is a **26,751,488 B standalone exe** (no DLLs beside it) — a NativeAOT exe on the same SDK. Expect the RSH sidecar AOT exe ≈ 20–30 MB. Must be measured on Windows (gate H-HID-1 follow-up). |
| Running the sidecar | **SANDBOX-BLOCKED:** Windows PE cannot execute on Linux. `version` / `install-driver` / `create` / `submit` round-trips are unproven until Windows. The protocol code paths are written; the driver calls are not. |

### 3.3 IPC latency (Linux proxy — same JSON-lines protocol, Rust client ↔ .NET stub)

Harness: `rust-recreation/spike/g1-hidmaestro/ipc-bench/` (Rust) spawns the
stub `ipc-stub/` (.NET, same parse/serialise work as the real sidecar, 5 µs
simulated submit). 10,000 back-to-back + 1,000 at 200 Hz cadence (5 ms
spacing). Linux pipes are a **proxy** for Windows named pipes/stdin-stdout —
same order of magnitude, must be re-measured on Windows (H-HID-3).

| Metric | Measured |
|---|---|
| Cold start (spawn `dotnet` → hello line) | **106 ms** (first run 180 ms) |
| Submit RTT, back-to-back, n=10,000 (200 warmup) | min 15.8 µs, **p50 20.5 µs, p99 50.2 µs**, mean 39.1 µs, max 46.4 ms (single scheduling/GC outlier) |
| Submit RTT, 200 Hz cadence, n=1,000 | min 19.3 µs, **p50 69.0 µs, p99 276.5 µs**, mean 83.1 µs, max 1.23 ms |
| Frame size on the wire | 84 bytes JSON |

**Budget check:** E2E SLO is p99 ≤ 10 ms. IPC p99 at 200 Hz is **0.28 ms**
— ~35× headroom. IPC is not the bottleneck; the Wi-Fi/engine path dominates.
No need for named pipes, binary framing, or shared-memory writes in v1.0.

### 3.4 SDK behaviour measured on Linux (real DLL, no driver)

The catalog + profile builders are pure managed code, so they run on Linux —
these are real measurements, not docs quotes:

| Check | Result |
|---|---|
| `HMContext` ctor | OK in 3 ms (prewarm failures swallowed, per docs) |
| `LoadDefaultProfiles()` | **336 profiles** in 1,036–1,494 ms (cold parse; prewarm hides it in practice) |
| `ProfileLoadIssues` | **1 refused:** `steam-controller-2` (`catalog:valve/steam-controller-2.json`, "a registry root needs a key under HKCU or HKLM and a value name", `$.valueSources.steamUpdater.roots[0]`). Explains 336 vs the documented 337. |
| Catalog totals | deployable 239/336; `RequiresUsbipBackend` 7; `BuildsItsOwnReports` 4; backends umdf2 329 / usbip 7 |
| `IsDriverInstalled` | `False` on Linux (expected — no driver store) |
| `CreateController` / `InstallDriver` / `SubmitState` | **SANDBOX-BLOCKED:** Windows-only (SetupAPI, PnP, driver store). Not attempted beyond the code path existing in the prototype. |

## 4. Wheel-profile verdict — NO generic wheel in the catalog; custom profile wins

Measured catalog enumeration (real DLL, §3.4):

- **Wheel profiles: 50.** Every deployable one is a **brand identity**:
  Logitech G25/G27/G29/G29-PS4/G920/G923-PS/G923-Xbox/G PRO-PS/G PRO-Xbox,
  Driving Force/GT/Pro, MOMO Racing, Formula Vibration; Thrustmaster
  T150/T248/T300RS/T500RS/T80/TX-Xbox; Fanatec CSL DD/Elite, ClubSport V2/V2.5
  (several `deployable=False`); MOZA R3/R5/R9/R12/R16, Asetek, Simagic,
  Cammus, PXN, HORI, Xbox 360 Wheel v1/v2, SteelSeries SRW-S1, etc.
  Using any of these presents RSH as that brand's hardware to games —
  **identity spoofing, not allowed.** The C# backend correctly refused to
  pick a default (empty profile id → refuse).
- **"Generic" profiles: 3 — all gamepads, none a wheel:**
  - `generic-dinput-gamepad` — "Generic DirectInput USB Gamepad", VID
    0x0079/PID 0x0006, Manufacturer "Generic", 32 buttons, **4 axes
    (X/Y/Z/Rz), NO Slider**, hat, Type=`gamepad`, descriptor 63 B.
    No handbrake slider; games see a **gamepad, not a wheel** — sims that
    expect a wheel may not accept it, and bindings/FFB behaviour differ.
  - `joypad-generic-hid`, `tinyusb-generic-gamepad` (32 buttons, 6 axes) —
    also gamepads.
- **Verdict: no suitable catalog wheel profile exists.** The choice is
  (i) a custom profile, or (ii) a brand profile (rejected). Custom wins.

**Custom profile — PROVEN to build (measured, Linux, real DLL):**
`HidDescriptorBuilder().Joystick()` + `AddAxis(X/Y/Z/Rx/Slider, 16)` +
`AddButtons(32)` + `AddHat(4)` → **124-byte descriptor in 3 ms**;
`HMProfileBuilder` (`rsh-wheel-custom`, Vendor "RSH", VID 0x1209/PID 0x0001,
"RSH Wheel", Type=`wheel`, `InputReportSize(26)`) → profile builds in
~161 ms first call; `Deployable=True`, `Backend=umdf2`, `Usbip=False`,
`OwnReports=False`, `ButtonCount=32`, `AxisCount=5`,
`AvailableAxes=[X,Y,Z,Rx,Slider]`, `HasHat=True`, descriptor round-trips
byte-identical (`FromProfile` rebuild equal). This is exactly the RSH wheel
shape (steering X, accel Y, brake Z, clutch Rx, handbrake Slider, 32
buttons) with **no brand identity**.

**G2 must verify on Windows (gate H-HID-2):** (a) the descriptor's real
input-report size vs the `InputReportSize(26)` claim — the C# 26-byte raw
report assumed 128 buttons; this profile has 32 buttons + hat, so the true
report is smaller; G2 must derive the raw-report layout from the actual
descriptor, not copy the C# 26-byte assumption; (b) `CreateController` with
the custom profile binds and games see a wheel; (c) `SubmitState` floats
vs `SubmitRawReport` bytes — pick per measurement (raw report carries all
32 buttons; `SubmitState`'s `HMButton` enum is gamepad-named, ~31 bits).
VID 0x1209 (pid.codes open VID) + PID 0x0001 are spike placeholders —
confirm the final VID/PID allocation in G2 (must not collide with a real
product).

## 5. Risks

| # | Risk | Severity | Note / mitigation |
|---|---|---|---|
| R-1 | **Elevated sidecar for the whole session (UAC every launch)** | High (UX) | §1 headline. DACL gives Everyone only read on the input section; the submitting process must be admin. Mitigation: UAC once per launch when HIDMaestro backend is active; vJoy fallback needs no prompt; document in installer + Output page. Do NOT work around with a scheduled task/service storing elevation. |
| R-2 | **Bus factor 1 + fast churn** | Medium | Single maintainer (`hifihedgehog`); 30 releases; v2.0.0→v2.1.0 in 6 days; frame delivery + signal section rewritten between releases. Mitigation: pin v2.1.0 + SHA-256 (§3.1); re-verify on every upgrade; vJoy fallback always shipped. |
| R-3 | **NativeAOT must build on Windows** | Medium | Cross-OS AOT unsupported (measured). Mitigation: Windows CI runner for the sidecar (H4); self-contained 95 MB publish as fallback (buildable on Linux). AOT JSON must be source-generated (IL3050/IL2026). |
| R-4 | **MSIX + admin driver install interaction** | Medium | MSIX container apps don't install drivers directly; the elevated sidecar is spawned from the MSIX app (UAC prompt) — unproven. Gate H-HID-5 on Windows; NSIS fallback is the H4 decision if MSIX blocks it. |
| R-5 | **Locally trusted self-signed cert** | Medium | `InstallDriver()` creates a self-signed code-signing cert in the machine trust store. Corporate/locked-down PCs may block it via policy. Mitigation: clear first-run messaging; failure → vJoy fallback + explanation. |
| R-6 | **Anti-cheat reaction** | Medium | User-mode driver is less exposed than a kernel driver, but per-game reaction is unverified. Gate H-HID-6 (spot-check ≥1 anti-cheat title). |
| R-7 | **HVCI-on install/create unmeasured** | Medium | Architecture says it should pass (only inbox kernel drivers), but not measured. Gate H-HID-1 (Win11 24H2, HVCI on, owner PC). |
| R-8 | **Sidecar as a second process** | Low | Crash/hang/kill of the sidecar must be detected (heartbeat + exit-code watch) and fall back to vJoy; the SDK removes the device on sidecar exit (≤10 s sweep) — G2 must handle the window where the device vanishes mid-session. Stdin/stdout death = immediate detection (EOF). |
| R-9 | **Custom-profile report-size assumption** | Low | §4: derive the raw-report layout from the real descriptor on Windows; don't copy the C# 26-byte constant. |
| R-10 | **ARM64** | Out of scope | D19 stands: ARM64 unsupported; vendor ARM64 binaries unvalidated on hardware. |

## 6. What G2 builds (SW-R6.3)

1. **`rsh-hm-sidecar`** (C#, .NET 10, `net10.0-windows`): productionise the
   spike prototype — source-generated JSON (AOT-safe), `HMContext` lifecycle,
   `InstallDriver()` with `AlreadyCurrent` fast path, custom
   `rsh-wheel-custom` profile (final VID/PID), `CreateController(profile,
   "rsh-wheel")`, `SubmitState` + `SubmitRawReport` (pick per Windows
   measurement), `PublishPid*` stubbed/documented only (FFB not in v1.0),
   `DriverVersion`/`TransportIndex` diagnostics, heartbeat + clean
   `Dispose` (device removal), structured errors (no admin / driver missing /
   profile refused / create failed) mapped to user-facing messages.
   Windows CI job publishes the NativeAOT exe; self-contained publish is the
   fallback. Sidecar + `HIDMaestro.Core.dll` ship beside the Tauri app
   (DLL hash verified at sidecar start against §3.1).
2. **`rsh-output-hidmaestro`** (Rust): sidecar process manager (spawn
   elevated, stdin/stdout JSON client, EOF/crash detection, restart policy:
   no silent respawn loop — one retry, then vJoy fallback), `IJoystickOutput`
   trait impl (`try_connect / disconnect / update / send_neutral`), frame
   mapping (reuse the C# mapping math: signed X centre 0.5, unsigned
   pedals/slider, buttons 1–32, >32 reject — D9), neutral on watchdog/stop,
   backend selector + runtime switch (G4), diagnostics (driver version,
   sidecar latency via `OnSubmitLatencyMicros` passthrough, HVCI state).
3. **Tests:** 78-test equivalent suite (C# `RSH.HIDMaestro.Tests` cases
   re-expressed in Rust — mapping, validation, refusal rules, fallback);
   rig tests with a stubbed sidecar; Windows-gate tests on the owner PC.
4. **Gates before G2 is done:** H-HID-1 (HVCI-on install/create/remove),
   H-HID-2 (8-game matrix on the custom wheel profile), H-HID-3 (E2E SLO +
   4 h soak, IPC re-measured on Windows), H-HID-4 (sleep/resume), H-HID-5
   (MSIX/elevated-sidecar path), H-HID-6 (anti-cheat spot-check), H-HID-8
   (pinned version + SHA-256 in release notes + THIRD-PARTY-LICENSES.md).
   H-HID-7 (ARM64) stays unsupported.

## 7. Sandbox-blocked items (NOT verified — Windows PC required)

- Sidecar actually running: `install-driver` / `create` / `submit` round-trip
  against a live driver (Windows PE cannot execute here).
- `InstallDriver()` end-to-end (cert creation, signing, `pnputil`, driver
  store) and its timing; first-install vs `AlreadyCurrent` timings.
- Device visible to DirectInput/XInput/SDL3; game matrix; FFB output path.
- IPC latency on Windows (named pipes / stdin-stdout under Windows
  scheduling) — Linux proxy numbers (§3.3) are indicative only.
- HVCI-on behaviour; sleep/resume; MSIX interaction; anti-cheat reaction;
  NativeAOT exe size/startup on Windows.
- Custom-profile `CreateController` bind + real input-report size (§4).

## 8. Sources

- HIDMaestro docs: `hidmaestro.org/docs/reference/shared-memory-protocol/`,
  `/docs/reference/umdf2-driver-internals/`, `/docs/sdk/sdk-reference/`
  (all crawled 2026-11-14).
- Release: `github.com/hifihedgehog/HIDMaestro/releases` (v2.1.0,
  2026-10-27; API checked 2026-11-14 — still latest).
- C# reference behaviour: `RSH.Output.HIDMaestro` from the pre-recreation
  backup (`repo-rust-recreation-backup-2026-11-14.tar.gz`, extracted to
  `~/tmp` only, never committed); eval doc
  `rust-recreation/reference/docs/HIDMAESTRO-EVALUATION.md` (§2/§3/§4/§8).
- Prototype + bench source:
  `rust-recreation/spike/g1-hidmaestro/{sidecar,probe,ipc-bench,ipc-stub}/`.
