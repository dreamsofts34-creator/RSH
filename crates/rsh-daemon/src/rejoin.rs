//! Wi-Fi rejoin helper — join the PC to the RSH Wi-Fi itself
//! (LOGIC-FIX-PLAN §4.2, C-08; Phase 6). No-pairing redesign:
//! the board SSID is `RSH-XXXX` (D4 — last 4 hex of the board
//! Wi-Fi MAC, uppercase), derived from the observed HELLO; there
//! is no fixed `RSH` SSID anymore.
//!
//! Windows is the target (the user's PC): a user-level Wi-Fi
//! profile (`netsh wlan add profile … user=current`) + connect —
//! no admin needed. Linux is a documented stub: this host cannot
//! drive `NetworkManager` here, so [`rejoin_wifi`] returns the exact
//! manual steps instead of pretending to work.
//!
//! The Wi-Fi password (set by the user in Settings) is passed
//! through this module but never logged: log lines carry the SSID
//! only (see [`log_safe_summary`]). The profile XML is written to
//! a temp file, imported, then deleted immediately — the password
//! never stays on disk in cleartext.
//!
//! Wiring (no-pairing redesign): `main.rs` declares `mod rejoin;`.
//! [`DaemonApi::set_wifi_password`](crate::daemon_api::DaemonApi)
//! calls [`rejoin_wifi`] after the board password changes (D8 —
//! the WLAN profile update is part of the same IPC call), and
//! the first-connect flow calls [`rejoin_wifi_open`] to join the
//! board's open AP before the app connects. [`scan_rsh_networks`]
//! lists nearby `RSH-XXXX` hotspots for the wizard.
//!
//! Windows-only helpers (`escape_xml`, `build_profile_xml`,
//! `windows_steps`, `StepFailed`) are dead on the Linux build host
//! by design — they compile and run on the Windows target (the
//! release workflow's Windows build is the proof). The allowance
//! below keeps the Linux build warning-free without cfg-gating the
//! helpers and their tests apart.
#![allow(dead_code)]

use std::time::Duration;

/// What went wrong while rejoining the RSH Wi-Fi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejoinError {
    /// This platform has no automated rejoin (Linux here): the
    /// exact manual steps are in `instructions`.
    UnsupportedPlatform {
        /// What the user (or a later platform module) must run.
        instructions: String,
    },
    /// A step of the Windows flow failed: `step` names it
    /// (`add-profile` / `connect` / `write-profile` /
    /// `delete-profile` / `show-networks`), `detail`
    /// is the OS / netsh output (never the password).
    StepFailed {
        /// Which step failed.
        step: &'static str,
        /// OS / netsh detail, password-free.
        detail: String,
    },
}

impl RejoinError {
    /// Manual instructions for a platform without automated rejoin.
    #[must_use]
    pub fn unsupported(instructions: String) -> Self {
        Self::UnsupportedPlatform { instructions }
    }
}

impl std::fmt::Display for RejoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform { instructions } => write!(
                f,
                "Automatic Wi-Fi rejoin is not available on this platform. {instructions}"
            ),
            Self::StepFailed { step, detail } => {
                write!(f, "Wi-Fi rejoin failed at step '{step}': {detail}")
            }
        }
    }
}

impl std::error::Error for RejoinError {}

/// Escape text for an XML attribute/element value (`&`, `<`, `>`,
/// `"`, `'`).
#[must_use]
pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build the Windows WLAN profile XML for an SSID + WPA2-PSK key
/// (the format `netsh wlan add profile` imports). The password is
/// embedded only here, in the temp file — never in a log line or
/// in a command-line argument.
#[must_use]
pub fn build_profile_xml(ssid: &str, password: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\n\t<name>{ssid}</name>\n\t<SSIDConfig>\n\t\t<SSID>\n\t\t\t<name>{ssid}</name>\n\t\t</SSID>\n\t</SSIDConfig>\n\t<connectionType>ESS</connectionType>\n\t<connectionMode>manual</connectionMode>\n\t<MSM>\n\t\t<security>\n\t\t\t<authEncryption>\n\t\t\t\t<authentication>WPA2PSK</authentication>\n\t\t\t\t<encryption>AES</encryption>\n\t\t\t\t<useOneX>false</useOneX>\n\t\t\t</authEncryption>\n\t\t\t<sharedKey>\n\t\t\t\t<keyType>passPhrase</keyType>\n\t\t\t\t<protected>false</protected>\n\t\t\t\t<keyMaterial>{password}</keyMaterial>\n\t\t\t</sharedKey>\n\t\t</security>\n\t</MSM>\n</WLANProfile>\n",
        ssid = escape_xml(ssid),
        password = escape_xml(password),
    )
}

/// Exact Windows steps, in order, for docs / diagnostics. The
/// commands run as the current user — no admin, no UAC prompt
/// (user-level profile, confirmed by the Phase 0 spike).
#[must_use]
pub fn windows_steps() -> Vec<&'static str> {
    vec![
        "Open AP only (R3-FW-13): delete any stale profile for the SSID before importing the open profile, so a leftover WPA2 profile cannot block the join",
        "Write the WPA2-PSK profile XML to a unique %TEMP%\\rsh-wifi-profile-<pid>-<nanos>.xml (deleted on every exit path)",
        "netsh wlan add profile filename=\"<unique-temp-path>\" user=current (30 s timeout)",
        "netsh wlan connect name=RSH-XXXX (30 s timeout, SSID from the observed board MAC)",
        "Delete the unique temp profile XML (Drop guard)",
        "Confirm: the RSH Wi-Fi shows as Connected before starting the session",
    ]
}

/// Manual rejoin instructions for platforms without automation
/// (Linux): the exact command the user runs.
#[must_use]
pub fn manual_instructions(ssid: &str) -> String {
    format!(
        "Join the '{ssid}' Wi-Fi manually, then retry. Linux (NetworkManager): \
         nmcli device wifi connect '{ssid}' password '<the password you set in Settings>'. \
         While the board AP is still open (no password set yet), omit the password argument."
    )
}

/// Manual instructions for forgetting a network on platforms
/// without automation (Linux): the exact command the user runs.
#[must_use]
pub fn manual_forget_instructions(ssid: &str) -> String {
    format!(
        "Remove the '{ssid}' Wi-Fi network manually. Linux (NetworkManager): \
         nmcli connection delete '{ssid}'."
    )
}

/// Log-safe one-line summary of a rejoin attempt: SSID only.
/// The password must never appear in a log line, an error's
/// `detail`, or a command line passed to `netsh`.
#[must_use]
pub fn log_safe_summary(ssid: &str) -> String {
    format!("rejoin: ssid={ssid}")
}

/// Build the Windows WLAN profile XML for an open (unsecured)
/// SSID: `authentication` open, `encryption` none, no
/// `<sharedKey>` — the first-connect flow joins the board's open
/// AP with this profile ([`rejoin_wifi_open`]).
#[must_use]
pub fn build_open_profile_xml(ssid: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\n\t<name>{ssid}</name>\n\t<SSIDConfig>\n\t\t<SSID>\n\t\t\t<name>{ssid}</name>\n\t\t</SSID>\n\t</SSIDConfig>\n\t<connectionType>ESS</connectionType>\n\t<connectionMode>manual</connectionMode>\n\t<MSM>\n\t\t<security>\n\t\t\t<authEncryption>\n\t\t\t\t<authentication>open</authentication>\n\t\t\t\t<encryption>none</encryption>\n\t\t\t\t<useOneX>false</useOneX>\n\t\t\t</authEncryption>\n\t\t</security>\n\t</MSM>\n</WLANProfile>\n",
        ssid = escape_xml(ssid),
    )
}

/// Join the PC to the board's open Wi-Fi (first connect, D5):
/// writes the open profile XML and connects. Same
/// platform rules as [`rejoin_wifi`]: Windows via `netsh`,
/// any other OS returns [`RejoinError::UnsupportedPlatform`]
/// with the exact manual steps.
///
/// # Errors
///
/// [`RejoinError::StepFailed`] when a Windows step fails, or
/// [`RejoinError::UnsupportedPlatform`] off Windows.
pub fn rejoin_wifi_open(ssid: &str) -> Result<(), RejoinError> {
    #[cfg(target_os = "windows")]
    {
        rejoin_wifi_open_windows(ssid)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ssid;
        Err(RejoinError::unsupported(manual_instructions(ssid)))
    }
}

/// Join the PC to the board's Wi-Fi (WPA2-PSK).
///
/// This is also the password-change path (D8): importing a
/// profile whose `<name>` already exists overwrites it, so
/// calling this with the new password rewrites the stored
/// profile and reconnects — no separate delete step needed.
///
/// * Windows: writes the profile XML to `%TEMP%`, imports it as
///   a user-level profile, connects, deletes the temp file.
/// * Any other OS: returns [`RejoinError::UnsupportedPlatform`]
///   with the exact manual steps — the caller shows them to the
///   user instead of failing silently.
///
/// # Errors
///
/// [`RejoinError::StepFailed`] when a Windows step fails, or
/// [`RejoinError::UnsupportedPlatform`] off Windows.
pub fn rejoin_wifi(ssid: &str, password: &str) -> Result<(), RejoinError> {
    #[cfg(target_os = "windows")]
    {
        rejoin_wifi_windows(ssid, password)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (ssid, password);
        Err(RejoinError::unsupported(manual_instructions(ssid)))
    }
}

/// Parse `netsh wlan show interfaces` output into the connection
/// state (R3-D-09): `(connected, ssid)`. Lines look like
/// `State                  : connected` and
/// `SSID                   : RSH-9A55`. Pure + tested — the
/// Windows confirm step below only runs the command and hands its
/// stdout here.
#[must_use]
pub fn parse_interface_state(output: &str) -> (bool, Option<String>) {
    let mut connected = false;
    let mut ssid: Option<String> = None;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "State" => connected = value.trim().eq_ignore_ascii_case("connected"),
            "SSID" => {
                let name = value.trim().to_string();
                if !name.is_empty() {
                    ssid = Some(name);
                }
            }
            _ => {}
        }
    }
    (connected, ssid)
}

/// Parse `netsh wlan show networks` output into the visible
/// `RSH-XXXX` SSIDs (D4): lines look like `SSID 1 : RSH-9A55`.
/// Pure + tested — the Windows scan below only runs the command
/// and hands its stdout here.
#[must_use]
pub fn parse_rsh_ssids(output: &str) -> Vec<String> {
    let mut ssids = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("SSID") else {
            continue;
        };
        // `SSID 1 : RSH-9A55` — skip the number, take the name
        // after the colon.
        let Some((_, name)) = rest.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        if name.starts_with("RSH-") && !ssids.contains(&name) {
            ssids.push(name);
        }
    }
    ssids
}

/// Scan for nearby board hotspots (D4): SSIDs starting with
/// `RSH-`.
///
/// * Windows: `netsh wlan show networks`, parsed by
///   [`parse_rsh_ssids`].
/// * Any other OS: empty — there is no `netsh` here, and the
///   signature has no error channel; the wizard shows no
///   networks instead of failing.
#[must_use]
pub fn scan_rsh_networks() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("netsh");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.args(["wlan", "show", "networks"]);
        let Ok(output) = run_command_timeout(&mut cmd, NETSH_TIMEOUT, "show-networks") else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        parse_rsh_ssids(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Forget a board Wi-Fi network (D19, PC side of
/// `clear_wifi_password`): deletes the Windows WLAN profile for
/// `ssid` so the PC no longer auto-joins with a stale password.
/// Off Windows there is no daemon-managed profile, so this is a
/// no-op success (the user removes a manually saved network with
/// [`manual_forget_instructions`]).
///
/// # Errors
///
/// [`RejoinError::StepFailed`] when the Windows delete fails.
// The Windows branch returns `Err` (netsh delete failure); the
// `Result` is the cross-platform contract — on Linux this is
// always `Ok(())`, which is what `clippy::unnecessary_wraps`
// (correctly, for this target) complains about.
#[allow(clippy::unnecessary_wraps)]
pub fn forget_wifi_profile(ssid: &str) -> Result<(), RejoinError> {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("netsh");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.args(["wlan", "delete", "profile", &format!("name={ssid}")]);
        let output = run_command_timeout(&mut cmd, NETSH_TIMEOUT, "delete-profile")?;
        if !output.status.success() {
            return Err(RejoinError::StepFailed {
                step: "delete-profile",
                detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = ssid;
        Ok(())
    }
}

/// Hard timeout for one `netsh` call (DS-15): a hung Wi-Fi stack
/// used to block the manager loop indefinitely on an unbounded
/// `.output()`. 30 s per call, killed on expiry.
const NETSH_TIMEOUT: Duration = Duration::from_secs(30);

/// DS-45: unique temp path for the password-bearing profile XML:
/// `%TEMP%\rsh-wifi-profile-<pid>-<nanos>.xml`. The old fixed path
/// (`%TEMP%\rsh-wifi-profile.xml`) collided between two daemons /
/// users and left the password on disk if the process died between
/// write and delete.
#[must_use]
pub fn profile_temp_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "rsh-wifi-profile-{}-{nanos}.xml",
        std::process::id()
    ))
}

/// DS-45: deletes the profile file on drop, so the password-bearing
/// XML is removed on EVERY exit path — including a panic between
/// write and the explicit delete.
struct ProfileFileGuard(std::path::PathBuf);

impl Drop for ProfileFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Run one command with a hard timeout (DS-15): poll `try_wait()`
/// every 50 ms; on timeout kill the child and report `StepFailed`.
/// Stdout/stderr are piped and drained after exit — `netsh` writes
/// at most a few hundred bytes, far under the pipe buffer, so the
/// child cannot block on a full pipe while we poll. Platform-free
/// (std only) so the Linux suite can cover the timeout itself with
/// `/bin/sleep`.
fn run_command_timeout(
    cmd: &mut std::process::Command,
    timeout: Duration,
    step: &'static str,
) -> Result<std::process::Output, RejoinError> {
    use std::io::Read as _;
    use std::time::Instant;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| RejoinError::StepFailed {
            step,
            detail: format!("could not run netsh: {error}"),
        })?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(RejoinError::StepFailed {
                        step,
                        detail: format!("netsh timed out after {} s", timeout.as_secs()),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(RejoinError::StepFailed {
                    step,
                    detail: format!("could not wait for netsh: {error}"),
                });
            }
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr);
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Confirm the PC actually joined `ssid` (R3-D-09): `netsh wlan
/// connect` only means the request was accepted — association
/// takes seconds and can fail after exit 0. Polls `netsh wlan
/// show interfaces` (5 s per call) for up to ~15 s until the
/// interface reports connected to this SSID.
///
/// # Errors
///
/// [`RejoinError::StepFailed`] (`"confirm-connected"`) when the
/// interface never reports connected to `ssid` in the window.
#[cfg(target_os = "windows")]
fn confirm_connected_to(ssid: &str) -> Result<(), RejoinError> {
    // 15 × 1 s: association on a local AP normally lands in 1–3 s;
    // 15 s bounds a slow stack without stalling the caller
    // (the whole rejoin already runs off the manager loop).
    const CONFIRM_ATTEMPTS: u32 = 15;
    const CONFIRM_INTERVAL: Duration = Duration::from_secs(1);
    for _ in 0..CONFIRM_ATTEMPTS {
        let mut cmd = std::process::Command::new("netsh");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.args(["wlan", "show", "interfaces"]);
        if let Ok(output) =
            run_command_timeout(&mut cmd, Duration::from_secs(5), "confirm-connected")
            && output.status.success()
        {
            let (connected, current) =
                parse_interface_state(&String::from_utf8_lossy(&output.stdout));
            if connected && current.as_deref() == Some(ssid) {
                return Ok(());
            }
        }
        std::thread::sleep(CONFIRM_INTERVAL);
    }
    Err(RejoinError::StepFailed {
        step: "confirm-connected",
        detail: format!(
            "Wi-Fi did not connect to '{ssid}' within {} s — the network may be out of range or the board AP may be down",
            CONFIRM_ATTEMPTS * CONFIRM_INTERVAL.as_secs() as u32
        ),
    })
}

/// Delete the Windows WLAN profile for `ssid`, ignoring every
/// failure (R3-FW-13): a profile that does not exist fails the
/// delete, and that is fine. Used before importing an OPEN
/// profile — after a KEY_RESET the board AP is open, but the PC
/// may still hold a stale WPA2 profile for the same `RSH-XXXX`
/// SSID, which would make Windows try the old password against
/// an open AP instead of joining it.
#[cfg(target_os = "windows")]
fn delete_profile_best_effort(ssid: &str) {
    let mut cmd = std::process::Command::new("netsh");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.args(["wlan", "delete", "profile", &format!("name={ssid}")]);
    let _ = run_command_timeout(&mut cmd, NETSH_TIMEOUT, "delete-profile");
}

/// Windows implementation of [`rejoin_wifi`]: profile import +
/// connect via `netsh`. Never logs the password and never passes
/// it on a command line (it travels only inside the temp XML).
#[cfg(target_os = "windows")]
fn rejoin_wifi_windows(ssid: &str, password: &str) -> Result<(), RejoinError> {
    rejoin_wifi_windows_with_xml(ssid, &build_profile_xml(ssid, password), false)
}

/// Windows implementation of [`rejoin_wifi_open`]: open-profile
/// import + connect via `netsh` (no password anywhere).
#[cfg(target_os = "windows")]
fn rejoin_wifi_open_windows(ssid: &str) -> Result<(), RejoinError> {
    rejoin_wifi_windows_with_xml(ssid, &build_open_profile_xml(ssid), true)
}

/// Shared Windows flow for both profile kinds: write the XML to a
/// unique temp file, import it as a user-level profile, connect,
/// delete the temp file (Drop guard — every exit path).
///
/// `delete_stale_first` (R3-FW-13, open AP only): delete any
/// existing WLAN profile for the SSID before importing — after a
/// KEY_RESET the board AP is open, but the PC may still hold a
/// stale WPA2 profile for the same SSID. The WPA2 path keeps the
/// existing profile (importing overwrites it, and deleting first
/// would lose it if the import then failed).
#[cfg(target_os = "windows")]
fn rejoin_wifi_windows_with_xml(
    ssid: &str,
    xml: &str,
    delete_stale_first: bool,
) -> Result<(), RejoinError> {
    use std::io::Write as _;
    // R2-03 (S-05): `CommandExt` for the CREATE_NO_WINDOW flags on
    // the `netsh` helpers below.
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    // DS-45: unique temp path + Drop guard — the old fixed path
    // collided between processes and left the password on disk if
    // the daemon died between write and delete.
    let profile_path = profile_temp_path();
    {
        let mut file =
            std::fs::File::create(&profile_path).map_err(|e| RejoinError::StepFailed {
                step: "write-profile",
                detail: format!("could not write profile XML: {e}"),
            })?;
        file.write_all(xml.as_bytes())
            .map_err(|e| RejoinError::StepFailed {
                step: "write-profile",
                detail: format!("could not write profile XML: {e}"),
            })?;
    }
    let _guard = ProfileFileGuard(profile_path.clone());
    // R3-FW-13: before importing an OPEN profile, drop any stale
    // profile for this SSID (best effort — a missing profile is
    // not an error). A stale WPA2 profile would otherwise make
    // Windows offer the old password to an open AP.
    if delete_stale_first {
        delete_profile_best_effort(ssid);
    }
    // DS-15: each netsh call has a hard 30 s timeout (killed on
    // expiry) instead of the old unbounded `.output()` — and the
    // whole rejoin runs off the manager loop (see main.rs), so a
    // hung Wi-Fi stack stalls nothing.
    let mut add_cmd = std::process::Command::new("netsh");
    // R2-03 (S-05): no console window for the `netsh` helper.
    #[cfg(windows)]
    add_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    add_cmd.args([
        "wlan",
        "add",
        "profile",
        &format!("filename=\"{}\"", profile_path.display()),
        "user=current",
    ]);
    let add = run_command_timeout(&mut add_cmd, NETSH_TIMEOUT, "add-profile")?;
    if !add.status.success() {
        return Err(RejoinError::StepFailed {
            step: "add-profile",
            detail: String::from_utf8_lossy(&add.stderr).trim().to_string(),
        });
    }
    let mut connect_cmd = std::process::Command::new("netsh");
    // R2-03 (S-05): no console window for the `netsh` helper.
    #[cfg(windows)]
    connect_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    connect_cmd.args(["wlan", "connect", &format!("name={ssid}")]);
    let connect = run_command_timeout(&mut connect_cmd, NETSH_TIMEOUT, "connect")?;
    if !connect.status.success() {
        return Err(RejoinError::StepFailed {
            step: "connect",
            detail: String::from_utf8_lossy(&connect.stderr).trim().to_string(),
        });
    }
    // R3-D-09: `netsh wlan connect` exit 0 only means the request
    // was accepted — confirm the interface is actually connected
    // to this SSID before reporting success.
    confirm_connected_to(ssid)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_xml_is_wpa2_psk_with_escaped_key() {
        let xml = build_profile_xml("RSH-9A55", "p<ass&\"word'");
        assert!(
            xml.contains("<authentication>WPA2PSK</authentication>"),
            "{xml}"
        );
        assert!(xml.contains("<encryption>AES</encryption>"), "{xml}");
        assert!(xml.contains("<name>RSH-9A55</name>"), "{xml}");
        // The password is present, fully escaped — never raw.
        assert!(
            xml.contains("<keyMaterial>p&lt;ass&amp;&quot;word&apos;</keyMaterial>"),
            "{xml}"
        );
        assert!(!xml.contains("p<ass"), "{xml}");
    }

    #[test]
    fn escape_xml_covers_all_five() {
        assert_eq!(escape_xml("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
        assert_eq!(escape_xml("plain123"), "plain123");
    }

    #[test]
    fn windows_steps_are_exact_and_user_level() {
        let steps = windows_steps();
        assert!(steps.len() >= 4);
        assert!(steps.iter().any(|s| s.contains("netsh wlan add profile")));
        assert!(steps.iter().any(|s| s.contains("user=current")));
        assert!(
            steps
                .iter()
                .any(|s| s.contains("netsh wlan connect name=RSH"))
        );
        // No step asks for admin.
        assert!(!steps.iter().any(|s| s.to_lowercase().contains("admin")));
    }

    #[test]
    fn log_summary_never_carries_the_password() {
        let summary = log_safe_summary("RSH-9A55");
        assert!(summary.contains("RSH-9A55"));
        assert!(!summary.contains("secret"), "{summary}");
    }

    #[test]
    fn manual_instructions_name_the_network_and_tool() {
        let text = manual_instructions("RSH-9A55");
        assert!(text.contains("RSH-9A55"), "{text}");
        assert!(text.contains("nmcli"), "{text}");
    }

    #[test]
    fn linux_rejoin_returns_manual_steps_not_silent_failure() {
        // On this Linux host the Windows path cannot run: the
        // helper must say so, with the exact manual command.
        #[cfg(not(target_os = "windows"))]
        {
            let err = rejoin_wifi("RSH-9A55", "TestPass123").unwrap_err();
            match err {
                RejoinError::UnsupportedPlatform { instructions } => {
                    assert!(instructions.contains("nmcli"), "{instructions}");
                    assert!(instructions.contains("RSH"), "{instructions}");
                }
                RejoinError::StepFailed { .. } => {
                    panic!("expected UnsupportedPlatform, got StepFailed")
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            // Windows-only path: not exercised on this host.
        }
    }

    #[test]
    fn open_profile_xml_has_no_shared_key() {
        let xml = build_open_profile_xml("RSH-9A55");
        assert!(xml.contains("<name>RSH-9A55</name>"), "{xml}");
        assert!(
            xml.contains("<authentication>open</authentication>"),
            "{xml}"
        );
        assert!(xml.contains("<encryption>none</encryption>"), "{xml}");
        assert!(!xml.contains("sharedKey"), "{xml}");
        assert!(!xml.contains("keyMaterial"), "{xml}");
    }

    #[test]
    fn parse_interface_state_reads_connected_ssid() {
        // R3-D-09: the confirm step's parser — real `netsh wlan
        // show interfaces` shape (leading spaces, `key : value`).
        let connected = "There is 1 interface on the system:\n    Name                   : Wi-Fi\n    State                  : connected\n    SSID                   : RSH-9A55\n    BSSID                  : aa:bb:cc:dd:ee:ff\n";
        assert_eq!(
            parse_interface_state(connected),
            (true, Some("RSH-9A55".to_string()))
        );
        // Connected to a different network: not our board.
        let other =
            "    State                  : connected\n    SSID                   : HomeWifi\n";
        assert_eq!(
            parse_interface_state(other),
            (true, Some("HomeWifi".to_string()))
        );
        // Disconnected: no SSID to report.
        let down = "    State                  : disconnected\n";
        assert_eq!(parse_interface_state(down), (false, None));
        // Empty output: not connected.
        assert_eq!(parse_interface_state(""), (false, None));
    }

    #[test]
    fn parse_rsh_ssids_keeps_only_rsh_networks() {
        let output = "Interface name : Wi-Fi\n\
            There are 3 networks currently visible.\n\
            SSID 1 : RSH-9A55\n\
            \tNetwork type            : Infrastructure\n\
            SSID 2 : HomeWifi\n\
            \tNetwork type            : Infrastructure\n\
            SSID 3 : RSH-EEFF\n";
        assert_eq!(
            parse_rsh_ssids(output),
            vec!["RSH-9A55".to_string(), "RSH-EEFF".to_string()]
        );
        assert_eq!(parse_rsh_ssids("no networks here"), Vec::<String>::new());
        // A bare `RSH` (no dash) is not a board hotspot.
        assert_eq!(parse_rsh_ssids("SSID 1 : RSH\n"), Vec::<String>::new());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn linux_scan_returns_empty_and_forget_is_noop() {
        assert_eq!(scan_rsh_networks(), Vec::<String>::new());
        assert!(forget_wifi_profile("RSH-9A55").is_ok());
        assert!(rejoin_wifi_open("RSH-9A55").is_err());
    }

    #[test]
    fn step_failed_displays_without_secret() {
        let err = RejoinError::StepFailed {
            step: "connect",
            detail: "network not found".to_string(),
        };
        let text = err.to_string();
        assert!(text.contains("connect"), "{text}");
        assert!(text.contains("network not found"), "{text}");
    }

    #[test]
    fn profile_temp_path_is_unique_per_call() {
        // DS-45: the fixed `%TEMP%\rsh-wifi-profile.xml` path
        // collided between processes; each call must name a
        // different file, and it must carry the pid.
        let first = profile_temp_path();
        let second = profile_temp_path();
        assert_ne!(first, second, "two calls must not share a path");
        assert!(
            first
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("rsh-wifi-profile")),
            "{first:?}"
        );
        assert!(
            first.file_name().is_some_and(|name| name
                .to_string_lossy()
                .contains(&std::process::id().to_string())),
            "the path must carry the pid: {first:?}"
        );
    }

    #[test]
    fn profile_file_guard_deletes_on_drop() {
        // DS-45: the password-bearing XML must not survive any
        // exit path, including a panic — the Drop guard is the
        // mechanism.
        let path = profile_temp_path();
        std::fs::write(&path, "secret").expect("write temp profile");
        assert!(path.exists());
        {
            let _guard = ProfileFileGuard(path.clone());
        }
        assert!(!path.exists(), "guard must delete the file on drop");
    }

    #[test]
    #[cfg(unix)]
    fn run_command_timeout_kills_a_hung_command() {
        // DS-15: the timeout itself is platform-free (std only),
        // so it is covered here with /bin/sleep instead of netsh.
        let mut sleep = std::process::Command::new("/bin/sleep");
        sleep.arg("30");
        let start = std::time::Instant::now();
        let error = run_command_timeout(&mut sleep, Duration::from_millis(200), "connect")
            .expect_err("a 30 s sleep must time out at 200 ms");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must bound the call: {:?}",
            start.elapsed()
        );
        match error {
            RejoinError::StepFailed { step, detail } => {
                assert_eq!(step, "connect");
                assert!(detail.contains("timed out"), "{detail}");
            }
            RejoinError::UnsupportedPlatform { instructions } => {
                panic!("expected StepFailed timeout, got UnsupportedPlatform: {instructions}")
            }
        }
        // And a fast command still succeeds with its output.
        let mut echo = std::process::Command::new("/bin/echo");
        echo.arg("ok");
        let output = run_command_timeout(&mut echo, Duration::from_secs(5), "add-profile")
            .expect("echo must succeed");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
    }
}
