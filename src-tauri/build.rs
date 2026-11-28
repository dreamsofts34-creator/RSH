//! Build script for the RSH Tauri shell (generates the Tauri context from
//! `tauri.conf.json` and the capability files).
//!
//! v2.0.19: the HIDMaestro sidecar check is deleted with the backend —
//! no staged binary is a build input any more.

fn main() {
    tauri_build::build();
}
