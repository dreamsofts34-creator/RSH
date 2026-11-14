//! Build script for the RSH Tauri shell (generates the Tauri context from
//! `tauri.conf.json` and the capability files).

fn main() {
    tauri_build::build();
}
