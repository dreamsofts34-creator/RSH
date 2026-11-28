//! Contract self-checks for the Round 2C rig scenarios (OTA end to
//! end, upgrade preservation, installer data-dir safety). Each
//! function returns `(pass, detail)` for `harness::run_contract`.
//!
//! The OTA checks run the real app-core flash flow
//! (`rsh_app_core::webota::flash_firmware_with_pubkey`) against a
//! [`FakeBoard`](crate::webota_board::FakeBoard) over loopback TCP —
//! no real ESP32, no real Wi-Fi. Images are signed with throwaway
//! keys only; the production key never enters these checks.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rsh_app_core::webota::{
    FlashStage, WebOtaTarget, flash_firmware_with_pubkey, wait_for_board_version,
};
use rsh_profiles::store::ProfileStore;
use rsh_security::firmware_verify::FIRMWARE_IMAGE_MIN_LEN;

use crate::webota_board::{
    FailMode, FakeBoard, OTHER_TEST_SEED, build_test_image, build_test_image_with_seed, test_pubkey,
};

/// Exact-equality check for tuning values read back from JSON: the
/// values are written as short decimals and parsed back to the same
/// bits, but a strict `==` on `f64` is what `clippy::pedantic`
/// (rightly) complains about — an epsilon comparison says what is
/// meant (the value survived, not the bit pattern).
fn f64_eq(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}
fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rsh-rig-ota-{name}-{}-{}.bin",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

/// Write `image` to a temp file and return its path.
fn write_image(name: &str, image: &[u8]) -> PathBuf {
    let path = temp_path(name);
    std::fs::write(&path, image).expect("write test image");
    path
}

/// Run the flash flow against `board` and return the result plus the
/// upload percents reported.
fn run_flash(
    image_path: &Path,
    board: &FakeBoard,
    installed: (u8, u8, u8),
) -> (rsh_app_core::webota::FlashResult, Vec<u8>) {
    let target =
        WebOtaTarget::on_port(IpAddr::from([127, 0, 0, 1]), board.port(), board.password());
    let mut percents = Vec::new();
    let mut confirm = || wait_for_board_version(&target, "4.0.3", Duration::from_secs(10));
    let result = flash_firmware_with_pubkey(
        image_path,
        &target,
        installed,
        &test_pubkey(),
        &mut |progress| {
            if progress.stage == FlashStage::Uploading
                && let Some(percent) = progress.percent
            {
                percents.push(percent);
            }
        },
        &mut confirm,
    );
    (result, percents)
}

/// `ota-e2e-success`: a good signed 4.0.3 image flashes, the board
/// restarts on the new version, and the flow confirms 4.0.3.
#[must_use]
pub fn self_check_ota_e2e_success() -> (bool, String) {
    let board = FakeBoard::start("secret", (4, 0, 1));
    let (image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    let path = write_image("success", &image);
    let (result, percents) = run_flash(&path, &board, (4, 0, 1));
    let _ = std::fs::remove_file(&path);
    if !result.success {
        return (false, format!("flash failed: {:?}", result.error));
    }
    if result.version.as_deref() != Some("4.0.3") {
        return (
            false,
            format!("confirmed version wrong: {:?}", result.version),
        );
    }
    if board.version() != (4, 0, 3) {
        return (false, format!("board still on {:?}", board.version()));
    }
    if percents.last() != Some(&100) {
        return (false, format!("upload never reached 100%: {percents:?}"));
    }
    (
        true,
        format!(
            "flash OK: 4.0.1 -> 4.0.3 confirmed, board on 4.0.3, upload reached 100% ({} reports)",
            percents.len()
        ),
    )
}

/// `ota-e2e-same-version`: reinstalling the version the board already
/// runs (4.0.3 over 4.0.3) is allowed — the board rule in
/// `ota_validate.h` and the app-side verifier both reject only
/// *older* images, never an equal one. The flash succeeds, the
/// confirm reads 4.0.3, and the board stays on 4.0.3.
#[must_use]
pub fn self_check_ota_e2e_same_version_reinstall() -> (bool, String) {
    let board = FakeBoard::start("secret", (4, 0, 3));
    let (image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    let path = write_image("same-version", &image);
    let (result, percents) = run_flash(&path, &board, (4, 0, 3));
    let _ = std::fs::remove_file(&path);
    if !result.success {
        return (
            false,
            format!("same-version reinstall failed: {:?}", result.error),
        );
    }
    if result.version.as_deref() != Some("4.0.3") {
        return (
            false,
            format!("confirmed version wrong: {:?}", result.version),
        );
    }
    if board.version() != (4, 0, 3) {
        return (
            false,
            format!(
                "board moved to {:?} after a same-version reinstall",
                board.version()
            ),
        );
    }
    if percents.last() != Some(&100) {
        return (false, format!("upload never reached 100%: {percents:?}"));
    }
    (
        true,
        format!(
            "reinstall OK: 4.0.3 over 4.0.3 confirmed, board stays on 4.0.3, upload reached 100% ({} reports)",
            percents.len()
        ),
    )
}

/// `ota-e2e-bad-signature`: (a) a tampered image is rejected by the
/// app before any byte reaches the board; (b) an image signed under
/// a different key passes the app-side check (called with that key)
/// but the board rejects it — the board stays on 4.0.1 in both cases.
#[must_use]
pub fn self_check_ota_e2e_bad_signature() -> (bool, String) {
    // (a) Tampered image — app-side reject, board never contacted
    // (the flow validates before uploading).
    let board = FakeBoard::start("secret", (4, 0, 1));
    let (mut image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    image[2000] ^= 0x01;
    let path = write_image("tampered", &image);
    let (result, _) = run_flash(&path, &board, (4, 0, 1));
    let _ = std::fs::remove_file(&path);
    if result.success {
        return (false, "tampered image flashed (must be rejected)".into());
    }
    let error = result.error.clone().unwrap_or_default();
    if !error.contains("signature") {
        return (
            false,
            format!("tampered image failed, but not on the signature: {error}"),
        );
    }
    if board.version() != (4, 0, 1) {
        return (
            false,
            format!(
                "board moved to {:?} after a tampered image",
                board.version()
            ),
        );
    }
    // (b) Wrong-key image — the app is told the image's key (so its
    // own check passes) but the board trusts only the test key.
    let (wrong_key_image, wrong_pubkey) =
        build_test_image_with_seed("4.0.3", FIRMWARE_IMAGE_MIN_LEN, &OTHER_TEST_SEED);
    let path = write_image("wrongkey", &wrong_key_image);
    let target =
        WebOtaTarget::on_port(IpAddr::from([127, 0, 0, 1]), board.port(), board.password());
    let mut confirm = || wait_for_board_version(&target, "4.0.3", Duration::from_secs(2));
    let result = flash_firmware_with_pubkey(
        &path,
        &target,
        (4, 0, 1),
        &wrong_pubkey,
        &mut |_| {},
        &mut confirm,
    );
    let _ = std::fs::remove_file(&path);
    if result.success {
        return (
            false,
            "wrong-key image flashed (board must reject it)".into(),
        );
    }
    let error = result.error.unwrap_or_default();
    if !error.contains("signature verify fail") {
        return (
            false,
            format!("wrong-key image failed, but not on the board signature check: {error}"),
        );
    }
    if board.version() != (4, 0, 1) {
        return (
            false,
            format!(
                "board moved to {:?} after a wrong-key image",
                board.version()
            ),
        );
    }
    (
        true,
        "tampered image rejected app-side (signature); wrong-key image rejected board-side (signature verify fail); board stayed on 4.0.1 in both cases".into(),
    )
}

/// `ota-e2e-disconnect-rollback`: the connection drops mid-upload —
/// the flash fails and the board keeps running 4.0.1; a retry with
/// the link back succeeds to 4.0.3 (the board survived the abort).
#[must_use]
pub fn self_check_ota_e2e_disconnect_rollback() -> (bool, String) {
    let board = FakeBoard::start("secret", (4, 0, 1));
    board.set_fail_mode(FailMode::DropMidUpload {
        after_bytes: 30_000,
    });
    let (image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    let path = write_image("disconnect", &image);
    let (result, _) = run_flash(&path, &board, (4, 0, 1));
    if result.success {
        let _ = std::fs::remove_file(&path);
        return (false, "flash succeeded despite the mid-upload drop".into());
    }
    if board.version() != (4, 0, 1) {
        let _ = std::fs::remove_file(&path);
        return (
            false,
            format!(
                "board moved to {:?} after an aborted upload (must stay 4.0.1)",
                board.version()
            ),
        );
    }
    // Retry with the link back: the board must still take the update.
    board.set_fail_mode(FailMode::None);
    let (retry, _) = run_flash(&path, &board, (4, 0, 1));
    let _ = std::fs::remove_file(&path);
    if !retry.success {
        return (
            false,
            format!("retry after the abort failed: {:?}", retry.error),
        );
    }
    if board.version() != (4, 0, 3) {
        return (
            false,
            format!("board on {:?} after a successful retry", board.version()),
        );
    }
    (
        true,
        "mid-upload drop failed the flash, board stayed on 4.0.1 (rollback); retry succeeded to 4.0.3".into(),
    )
}

/// `connect-migration-402-403`: a board still on the last
/// pairing-era firmware (4.0.2) flashes the signed 4.0.3 image
/// via the same `WebOTA` flow and confirms 4.0.3 (plan D6: the app
/// is updated first, the board is flashed with the old stored
/// Wi-Fi password, and 4.0.3 first boot deletes the old NVS
/// secrets and comes up on an open AP — the NVS deletion itself
/// is firmware behaviour, covered by the firmware host tests).
#[must_use]
pub fn self_check_connect_migration_402_403() -> (bool, String) {
    let board = FakeBoard::start("secret", (4, 0, 2));
    let (image, _) = build_test_image("4.0.3", FIRMWARE_IMAGE_MIN_LEN);
    let path = write_image("migration-402-403", &image);
    let (result, percents) = run_flash(&path, &board, (4, 0, 2));
    let _ = std::fs::remove_file(&path);
    if !result.success {
        return (false, format!("migration flash failed: {:?}", result.error));
    }
    if result.version.as_deref() != Some("4.0.3") {
        return (
            false,
            format!("confirmed version wrong: {:?}", result.version),
        );
    }
    if board.version() != (4, 0, 3) {
        return (false, format!("board still on {:?}", board.version()));
    }
    if percents.last() != Some(&100) {
        return (false, format!("upload never reached 100%: {percents:?}"));
    }
    (
        true,
        format!(
            "migration OK: 4.0.2 -> 4.0.3 confirmed, board on 4.0.3, upload reached 100% ({} reports)",
            percents.len()
        ),
    )
}

/// `upgrade-preserve`: a v2.0.0/v2.0.1-era store (schema 2, hand-written
/// JSON, missing the newest fields) opens intact in the new version —
/// profiles, tunings, active id and settings all survive, with no load
/// warnings — and a save + reopen cycle loses nothing.
#[must_use]
pub fn self_check_upgrade_preserve() -> (bool, String) {
    let dir = std::env::temp_dir().join(format!(
        "rsh-rig-upgrade-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return (false, format!("cannot create temp store dir: {error}"));
    }
    // Hand-written old store: field subset only (no steam_path /
    // iracing_path / calibration_firmware_version / calibrated_at_utc /
    // board_mac — those arrived later and must default, not break the
    // load). Values are distinctive (non-default) so "preserved" is
    // provable, not a default masquerading as a value.
    let profiles_json = r#"{
  "schema_version": 2,
  "profiles": [
    {
      "id": "gt3-setup",
      "profile": {
        "name": "GT3 Setup",
        "schema_version": 2,
        "steering_calibration": {"raw_min": 100, "raw_center": 1650, "raw_max": 3200, "is_set": true, "is_millivolts": true},
        "accel_calibration": {"raw_min": 80, "raw_max": 3100, "is_set": true, "is_millivolts": true},
        "brake_calibration": null,
        "clutch_calibration": null,
        "deadzone": 0.05,
        "curve_strength": -0.3,
        "smoothing": 0.5,
        "invert_steering": false,
        "invert_accel": false,
        "invert_brake": false,
        "combine_pedals": false,
        "gear_up_vjoy_button": 5,
        "gear_down_vjoy_button": 6,
        "pedal_press_ms": 120.0,
        "pedal_release_ms": 90.0,
        "steering_scale": 0.6,
        "game_process_name": "acc.exe",
        "car_model": "ferrari_488_gt3",
        "game": 2,
        "module_tunings": {
          "steering": {"deadzone": 0.05, "curve_strength": -0.3, "smoothing": 0.5, "invert": false, "scale": 0.6, "enabled": true},
          "brake": {"deadzone": 0.02, "curve_strength": 0.1, "smoothing": 0.2, "invert": false, "scale": 1.0, "enabled": true}
        },
        "needs_recalibration": false
      }
    },
    {
      "id": "drift",
      "profile": {
        "name": "Drift",
        "schema_version": 2,
        "deadzone": 0.0,
        "curve_strength": 0.4,
        "smoothing": 0.1,
        "game_process_name": "acs.exe",
        "car_model": "",
        "game": 1,
        "module_tunings": {
          "steering": {"deadzone": 0.0, "curve_strength": 0.4, "smoothing": 0.1, "invert": true, "scale": 1.0, "enabled": true},
          "handbrake": {"deadzone": 0.0, "curve_strength": 0.0, "smoothing": 0.0, "invert": false, "scale": 1.0, "enabled": true}
        },
        "needs_recalibration": false
      }
    }
  ],
  "active_profile_id": "gt3-setup",
  "auto_switch_locked": false
}"#;
    let settings_json = r#"{
  "theme": "Dark",
  "language": "ur",
  "autostart": true,
  "start_minimized": true,
  "close_to_tray": false,
  "output_enabled": false,
  "esp_ip": "192.168.4.100",
  "vjoy_device_id": 3,
  "output_backend": "HIDMaestro",
  "hid_maestro_profile_id": "hm-7",
  "log_level": "Debug",
  "wizard_completed": true,
  "first_run_done": true,
  "auto_switch_profiles": false,
  "modules": {
    "steering": true, "accelerator": true, "brake": true, "clutch": false,
    "handbrake": true, "shifter": true, "horn": true, "buttons": true,
    "encoders": true, "led": false
  }
}"#;
    if let Err(error) = std::fs::write(dir.join("profiles.json"), profiles_json)
        .and_then(|()| std::fs::write(dir.join("settings.json"), settings_json))
    {
        return (false, format!("cannot write old store files: {error}"));
    }
    let check = |store: &ProfileStore| -> Result<String, String> {
        if store.list().len() != 2 {
            return Err(format!("expected 2 profiles, got {}", store.list().len()));
        }
        let gt3 = store
            .get("gt3-setup")
            .ok_or_else(|| "GT3 Setup profile missing".to_string())?;
        if gt3.profile.name != "GT3 Setup"
            || !f64_eq(gt3.profile.deadzone, 0.05)
            || !f64_eq(gt3.profile.curve_strength, -0.3)
            || !f64_eq(gt3.profile.steering_scale, 0.6)
            || gt3.profile.gear_up_vjoy_button != 5
        {
            return Err(format!("GT3 Setup values changed: {:?}", gt3.profile));
        }
        let steering_cal = gt3
            .profile
            .steering_calibration
            .ok_or_else(|| "GT3 steering calibration lost".to_string())?;
        if steering_cal.raw_min != 100
            || steering_cal.raw_center != 1650
            || steering_cal.raw_max != 3200
        {
            return Err(format!(
                "GT3 steering calibration changed: {steering_cal:?}"
            ));
        }
        let tuning = gt3
            .profile
            .module_tunings
            .get("steering")
            .ok_or_else(|| "GT3 steering tuning lost".to_string())?;
        if !f64_eq(tuning.scale, 0.6) || !f64_eq(tuning.curve_strength, -0.3) {
            return Err(format!("GT3 steering tuning changed: {tuning:?}"));
        }
        let drift = store
            .get("drift")
            .ok_or_else(|| "Drift profile missing".to_string())?;
        // Profile-level invert_steering was never set in the old file
        // (defaults false); the tuning-level invert is the value that
        // must survive.
        let drift_steering = drift
            .profile
            .module_tunings
            .get("steering")
            .ok_or_else(|| "Drift steering tuning lost".to_string())?;
        if !f64_eq(drift.profile.curve_strength, 0.4) || !drift_steering.invert {
            return Err(format!("Drift values changed: {:?}", drift.profile));
        }
        if store.active().map(|p| p.id.as_str()) != Some("gt3-setup") {
            return Err("active profile is no longer gt3-setup".into());
        }
        let settings = store.settings();
        if settings.theme != "Dark"
            || settings.language != "ur"
            || !settings.autostart
            || settings.esp_ip != "192.168.4.100"
            || settings.vjoy_device_id != 3
            // v2.0.19: a persisted "HIDMaestro" backend migrates to
            // vJoy on load (the backend is deleted), and the
            // `hid_maestro_profile_id` setting is dropped with it.
            || settings.output_backend != "VJoy"
            || settings.log_level != "Debug"
            || !settings.wizard_completed
            || settings.auto_switch_profiles
            || settings.modules.get("clutch") != Some(&false)
            || settings.modules.get("led") != Some(&false)
        {
            return Err(format!("settings changed: {settings:?}"));
        }
        if !store.load_warnings().is_empty() {
            return Err(format!("load warnings: {:?}", store.load_warnings()));
        }
        Ok("2 profiles + settings intact, no load warnings".into())
    };
    let store = match ProfileStore::open(&dir) {
        Ok(store) => store,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return (false, format!("old store failed to open: {error}"));
        }
    };
    let first = check(&store);
    if let Err(detail) = first {
        let _ = std::fs::remove_dir_all(&dir);
        return (false, format!("open: {detail}"));
    }
    // Save + reopen: the upgrade must not lose anything through a
    // write cycle either (no migration rewrite dropping fields).
    if let Err(error) = store.save() {
        let _ = std::fs::remove_dir_all(&dir);
        return (false, format!("save after open failed: {error}"));
    }
    let reopened = match ProfileStore::open(&dir) {
        Ok(store) => store,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            return (false, format!("reopen after save failed: {error}"));
        }
    };
    let second = check(&reopened);
    let _ = std::fs::remove_dir_all(&dir);
    match second {
        Ok(detail) => (true, format!("{detail}; save + reopen cycle clean")),
        Err(detail) => (false, format!("reopen: {detail}")),
    }
}

/// `installer-preserves-data`: the COMMITTED NSIS script
/// (`installer/rsh.nsi` — extracted from the release workflow in
/// v2.0.13 P1-1; the workflow no longer generates it inline) never
/// touches the data dir (`%AppData%\RSH`) — it installs only under
/// `$INSTDIR`, and uninstall deletes only `$INSTDIR` / `$SMPROGRAMS`
/// paths (+ the desktop shortcut it creates itself). Also asserts
/// the signed OTA image is bundled in the installer, and that the
/// release workflow still builds this committed script (fail closed
/// if the workflow stops using it).
#[must_use]
pub fn self_check_installer_preserves_data() -> (bool, String) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root above tools/rsh-rig");
    // Fail closed: the workflow must still build the committed
    // script — the "Prepare NSIS script" step substitutes
    // ${VERSION} into installer/rsh.nsi and makensis builds it.
    let workflow = root.join(".github/workflows/release.yml");
    let workflow_text = match std::fs::read_to_string(&workflow) {
        Ok(text) => text,
        Err(error) => {
            return (
                false,
                format!("cannot read {}: {error}", workflow.display()),
            );
        }
    };
    if !workflow_text.contains("Prepare NSIS script") || !workflow_text.contains("rsh.nsi") {
        return (
            false,
            "release.yml no longer builds the committed installer/rsh.nsi".into(),
        );
    }
    let nsi_path = root.join("installer/rsh.nsi");
    let nsis = match std::fs::read_to_string(&nsi_path) {
        Ok(text) => text,
        Err(error) => {
            return (
                false,
                format!("cannot read {}: {error}", nsi_path.display()),
            );
        }
    };
    let nsis = nsis.as_str();
    // 1. No data-dir reference in any ACTIVE installer command.
    // Comment lines (leading `;`) are ignored: the script documents
    // the guarantee in a comment ("never touches ... %APPDATA%\RSH"),
    // which is not a data-dir access. `DetailPrint` lines are
    // ignored too: they are message text, not filesystem operations
    // (DS-69 tells the user to delete %APPDATA%\RSH manually — the
    // installer itself never touches it).
    let active_lines: String = nsis
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with(';') && !trimmed.starts_with("DetailPrint")
        })
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "APPDATA",
        "AppData",
        "LOCALAPPDATA",
        "%AppData%",
        ".local/share",
        "data_dir",
        "data dir",
    ] {
        if active_lines
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
        {
            return (
                false,
                format!(
                    "NSIS script mentions the data dir ({needle}) — an upgrade could wipe settings/profiles"
                ),
            );
        }
    }
    // 2. The signed OTA image is bundled (the daemon resolves it as
    // <exe dir>/firmware-signed.bin).
    if !nsis.contains("firmware-signed.bin") {
        return (
            false,
            "NSIS script does not bundle firmware-signed.bin".into(),
        );
    }
    // 3. Uninstall deletes only $INSTDIR / $SMPROGRAMS paths.
    let uninstall_marker = "Section \"Uninstall\"";
    let Some(uninstall_at) = nsis.find(uninstall_marker) else {
        return (false, "NSIS script has no Uninstall section".into());
    };
    let uninstall = &nsis[uninstall_at..];
    let uninstall_end = uninstall.find("SectionEnd").unwrap_or(uninstall.len());
    let uninstall = &uninstall[..uninstall_end];
    for line in uninstall.lines() {
        let line = line.trim();
        if line.starts_with("Delete ") || line.starts_with("RMDir ") {
            let target = line
                .trim_start_matches("Delete ")
                .trim_start_matches("RMDir ")
                .trim();
            // Allowed: install dir, Start Menu, and the ONE desktop
            // shortcut the installer creates itself
            // (`CreateShortcut "$DESKTOP\\RSH.lnk"` at install time)
            // — removing it is cleanup of the installer's own file,
            // not user data.
            if !target.starts_with("\"$INSTDIR")
                && !target.starts_with("\"$SMPROGRAMS")
                && target != "\"$DESKTOP\\\\RSH.lnk\""
            {
                return (
                    false,
                    format!("uninstall deletes a non-install path: {line}"),
                );
            }
        }
    }
    (
        true,
        "NSIS script touches no data-dir path (install + uninstall only $INSTDIR/$SMPROGRAMS/registry); firmware-signed.bin bundled".into(),
    )
}
