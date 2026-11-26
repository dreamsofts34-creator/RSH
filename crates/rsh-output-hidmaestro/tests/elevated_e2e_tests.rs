//! True end-to-end daemon→sidecar test (update4-fix-hid-e2e).
//!
//! Windows-only: the crate doc stays above the `cfg` so the crate
//! is documented even where the test is compiled out (Linux).
//!
//! The production path, exercised for real: a [`SidecarManager`] with
//! `elevated=true` spawns the staged sidecar via `ShellExecuteEx`
//! (`runas`) and the sidecar connects back over loopback TCP — the
//! exact path `SidecarManager::spawn_windows_elevated` takes in the
//! daemon. `tools/ci/check-sidecar-hello.ps1` exercises only the
//! sidecar side of that TCP path (its own raw `TcpListener`); this
//! test drives the manager side of it, so a regression in the
//! manager's spawn / accept / token-check / hello-validation code
//! fails here instead of on the user's PC (the v2.0.10 failure mode).
//!
//! Runs ONLY on Windows, and only when the sidecar is staged:
//! `test-windows` runs `tools/ci/stage-sidecar.ps1` BEFORE
//! `cargo test --workspace`, staging the exe to
//! `src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe`
//! with `HIDMaestro.Core.dll` beside it. This test resolves that
//! staged path from `CARGO_MANIFEST_DIR` (override with
//! `RSH_HM_SIDECAR_E2E` for a local Windows run against a different
//! build). If the staged exe (or its DLL) is missing the test FAILS
//! with an actionable message — it never skips: a skip would let
//! the E2E path silently drop out of CI and pass green while
//! untested (the v2.0.8 lesson).
//!
//! The hello test does NOT require the `HIDMaestro` driver to be
//! installed and never calls `install_driver` / `create`: the sidecar
//! sends hello BEFORE `HMContext` creation, so hello + DLL verdict +
//! admin prove the daemon→sidecar path without any driver. On the
//! CI runner the agent is already elevated, so `runas` launches
//! the sidecar without a UAC prompt.
//!
//! HID P2-5 (polish round): the driver path after hello
//! (`describe-profile` → `install-driver` → `create` → `submit` →
//! `dispose` — the exact post-hello sequence `backend.rs`
//! `connect_inner` runs) had NO automated proof; the e2e stopped
//! at hello/ping. `elevated_sidecar_driver_path_e2e` below proves
//! it, but it is OPT-IN (`RSH_E2E_DRIVER=1`): `install-driver`
//! installs a kernel driver on the machine, which the shared CI
//! runner must not do on every run. Run it on a dedicated Windows
//! machine (or a throwaway VM) with the driver not yet installed —
//! it asserts `now_installed`, a 15-byte `input_report_size`, a
//! created controller, and a neutral-frame submit round-trip.
//!
//! What `connect()` returning `Ok` proves, beyond the explicit
//! asserts below: the token echo — `validate_hello` compares the
//! sidecar's echoed token against the one the manager issued
//! (constant-time) and refuses a wrong/missing echo with
//! `BadHello`, so a successful connect IS the token-echo proof.
//! There is no separate token assert because `HelloInfo` carries
//! no token field by design (the token is a secret while the
//! sidecar is alive).
//!
//! v2.0.12 (H1): the token travels on the sidecar's command line
//! (`--token <nonce>`) — the old environment transport died on the
//! owner's PC (AIS drops env vars set by the non-elevated daemon
//! when it launches the sidecar elevated via `runas`). This test
//! exercises exactly that production path, so a regression in the
//! argv token handoff fails here instead of on the user's PC.
#![cfg(windows)]

use std::path::PathBuf;

use rsh_output_hidmaestro::manager::{HelloInfo, SidecarConfig, SidecarManager};
use rsh_output_hidmaestro::mapping::{CUSTOM_RAW_REPORT_SIZE, GamepadFrame};
use rsh_output_hidmaestro::protocol::Command;
use rsh_output_hidmaestro::{PINNED_DLL_SHA256, PINNED_VERSION};

/// The staged production sidecar, as `stage-sidecar.ps1` leaves it
/// (fail-closed: a missing exe or DLL is a test failure, never a
/// skip — see the module docs).
fn staged_sidecar_path() -> PathBuf {
    let path = match std::env::var("RSH_HM_SIDECAR_E2E") {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crate manifest is under <repo>/crates/rsh-output-hidmaestro")
            .join("src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe"),
    };
    assert!(
        path.is_file(),
        "staged sidecar exe not found at {} — run tools/ci/stage-sidecar.ps1 first \
         (CI test-windows stages it before `cargo test --workspace`); \
         override with RSH_HM_SIDECAR_E2E to point at another build",
        path.display()
    );
    let dll = path.with_file_name("HIDMaestro.Core.dll");
    assert!(
        dll.is_file(),
        "HIDMaestro.Core.dll not found beside the staged sidecar at {} — \
         stage-sidecar.ps1 stages the hash-verified DLL beside the exe; \
         without it the sidecar reports no DLL hash and hello is refused",
        dll.display()
    );
    path
}

#[test]
fn elevated_sidecar_connect_hello_e2e() {
    let mut cfg = SidecarConfig::with_path(staged_sidecar_path());
    // Production path, stated explicitly (not left to the default):
    // `runas` + loopback TCP connect-back.
    cfg.elevated = true;
    let mut manager = SidecarManager::new(cfg);
    let info: HelloInfo = manager
        .connect()
        .expect("elevated sidecar connect + hello over TCP connect-back");
    assert_eq!(
        info.sdk_version.as_deref(),
        Some(PINNED_VERSION),
        "hello sdk must be the pinned version"
    );
    assert!(info.admin, "sidecar must run elevated (admin=true)");
    assert_eq!(
        info.transport.as_deref(),
        // HID P3-6 (polish round): the sidecar reports the exact
        // transport name "tcp-loopback" (was "tcp"); the manager
        // records it verbatim from the hello reply.
        Some("tcp-loopback"),
        "production path is runas + TCP connect-back, not stdio"
    );
    assert_eq!(
        info.dll_sha256.as_deref(),
        Some(PINNED_DLL_SHA256),
        "hello must measure the pinned HIDMaestro.Core.dll hash"
    );
    assert_eq!(
        info.dll_sha256_ok,
        Some(true),
        "hello DLL-hash verdict must be OK"
    );
    assert!(manager.is_running());
    // The TCP session survives hello: a ping round-trips over the
    // same transport (write path, not just the hello read). No
    // driver call is made here — install/create are out of scope
    // for this test by design (see the module docs).
    let reply = manager
        .request(&Command::Ping)
        .expect("ping over the TCP transport after hello");
    assert!(reply.ok);
    assert_eq!(reply.pong, Some(true));
    manager.shutdown();
    assert!(!manager.is_running());
}

/// HID P2-5: the post-hello driver path, end to end — the exact
/// sequence `backend.rs` `connect_inner` runs after hello:
/// `describe-profile` (15-byte report size) → `install-driver` →
/// `create` → one neutral-frame `submit` → `dispose`.
///
/// OPT-IN ONLY (`RSH_E2E_DRIVER=1`): `install-driver` installs a
/// kernel driver on the machine. CI does NOT set the variable — the
/// shared runner must not install a driver on every run. Run it on
/// a dedicated Windows machine or a throwaway VM, elevated, with
/// the staged sidecar + DLL (same staging as the hello test).
/// A skip here is deliberate, not a hole: the hello test above stays
/// fail-closed and proves the spawn path on every CI run.
#[test]
fn elevated_sidecar_driver_path_e2e() {
    if std::env::var("RSH_E2E_DRIVER").as_deref() != Ok("1") {
        eprintln!("skipping driver-path e2e: set RSH_E2E_DRIVER=1 on a dedicated Windows machine");
        return;
    }
    let mut cfg = SidecarConfig::with_path(staged_sidecar_path());
    cfg.elevated = true;
    let mut manager = SidecarManager::new(cfg);
    let info: HelloInfo = manager
        .connect()
        .expect("elevated sidecar connect + hello over TCP connect-back");
    assert!(info.admin, "sidecar must run elevated (admin=true)");
    // describe-profile: the 15-byte raw-report layout must match
    // the Rust side (HID P2-1) — a mismatch fails the connect.
    let described = manager
        .describe_profile()
        .expect("describe-profile after hello");
    assert_eq!(
        described.input_report_size,
        Some(CUSTOM_RAW_REPORT_SIZE),
        "sidecar input_report_size must be the 15-byte custom layout"
    );
    // install-driver: the driver must be installed afterwards.
    let installed = manager
        .install_driver()
        .expect("install-driver (elevated, 120 s budget)");
    assert!(
        installed.now_installed,
        "driver must be installed after install-driver: {installed:?}"
    );
    // create: a controller exists and reports its driver version.
    let created = manager.create().expect("create custom wheel profile");
    assert!(
        created.transport_index.is_some(),
        "create must report a transport index: {created:?}"
    );
    assert!(
        created.driver_version.is_some(),
        "create must report a driver version: {created:?}"
    );
    // submit: one neutral frame round-trips to the created
    // controller (sync submit waits for the sidecar's reply, so
    // this proves the frame reached the driver, not just the queue).
    let latency_ms = manager
        .submit(GamepadFrame::neutral())
        .expect("neutral submit after create");
    assert!(
        (0.0..5_000.0).contains(&latency_ms),
        "submit latency must be sane: {latency_ms} ms"
    );
    // dispose: the controller is released cleanly.
    let disposed = manager.dispose().expect("dispose after submit");
    assert!(disposed.ok, "dispose must succeed: {disposed:?}");
    manager.shutdown();
    assert!(!manager.is_running());
}
