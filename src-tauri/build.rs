//! Build script for the RSH Tauri shell (generates the Tauri context from
//! `tauri.conf.json` and the capability files).
//!
//! Bug 7 (v1.0.2): the staged sidecar is a build input, but a fresh
//! clone has no `src-tauri/binaries/` at all — without the check below
//! the failure surfaced as Tauri's cryptic `ResourcePathNotFound` for
//! `binaries\rsh-hm-sidecar-...exe`. Fail here with the exact fix
//! instead. Windows-only: the sidecar is a Windows binary and Linux
//! builds never need it. CI and the release workflow stage it first
//! (`tools/ci/stage-sidecar.ps1`); a local Windows build stages it
//! with the same script — the sidecar is fetched + hash-verified at
//! build time and never committed (see `.gitignore`).

fn main() {
    if cfg!(windows) {
        let sidecar = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("rsh-hm-sidecar-x86_64-pc-windows-msvc.exe");
        assert!(
            sidecar.exists(),
            "HIDMaestro sidecar not staged: {} is missing. From the repo root, run: pwsh -NoProfile -ExecutionPolicy Bypass -File tools/ci/stage-sidecar.ps1 (needs the .NET 10 SDK; it fetches HIDMaestro v2.1.0, verifies the ZIP + DLL SHA-256, and publishes the NativeAOT sidecar). CI and the release workflow stage it automatically.",
            sidecar.display()
        );
    }
    tauri_build::build();
}
