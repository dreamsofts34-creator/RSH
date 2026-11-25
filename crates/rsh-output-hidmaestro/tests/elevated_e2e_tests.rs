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
//! The test does NOT require the `HIDMaestro` driver to be installed
//! and never calls `install_driver` / `create`: the sidecar sends
//! hello BEFORE `HMContext` creation, so hello + DLL verdict +
//! admin prove the daemon→sidecar path without any driver. On the
//! CI runner the agent is already elevated, so `runas` launches
//! the sidecar without a UAC prompt.
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
        Some("tcp"),
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
