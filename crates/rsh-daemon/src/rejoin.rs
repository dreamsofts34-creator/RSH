//! Wi-Fi rejoin helper — after pairing, join the PC to the RSH
//! Wi-Fi itself (LOGIC-FIX-PLAN §4.2, C-08; Phase 6).
//!
//! Windows is the target (the user's PC): a user-level Wi-Fi
//! profile (`netsh wlan add profile … user=current`) + connect —
//! no admin needed. Linux is a documented stub: this host cannot
//! drive `NetworkManager` here, so [`rejoin_wifi`] returns the exact
//! manual steps instead of pretending to work.
//!
//! The one-time Wi-Fi password is passed through this module but
//! never logged: log lines carry the SSID only (see
//! [`log_safe_summary`]). The profile XML is written to a temp
//! file, imported, then deleted immediately — the password never
//! stays on disk in cleartext.
//!
//! Wiring (Phase 6 + integration): `main.rs` declares `mod rejoin;`
//! and calls [`rejoin_wifi`] from the pairing-completion path in the
//! manager loop (password stashed while the window is open, used
//! once on the transition to Active).
//!
//! Windows-only helpers (`escape_xml`, `build_profile_xml`,
//! `windows_steps`, `StepFailed`) are dead on the Linux build host
//! by design — they compile and run on the Windows target (the
//! release workflow's Windows build is the proof). The allowance
//! below keeps the Linux build warning-free without cfg-gating the
//! helpers and their tests apart.
#![allow(dead_code)]

use std::time::Duration;

/// The board's `SoftAP` SSID the PC joins after pairing.
pub const RSH_SSID: &str = "RSH";

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
    /// (`add-profile` / `connect` / `write-profile`), `detail`
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
        "Write the WPA2-PSK profile XML to a unique %TEMP%\\rsh-wifi-profile-<pid>-<nanos>.xml (deleted on every exit path)",
        "netsh wlan add profile filename=\"<unique-temp-path>\" user=current (30 s timeout)",
        "netsh wlan connect name=RSH (30 s timeout)",
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
         nmcli device wifi connect '{ssid}' password '<one-time-password>'. \
         The one-time password was shown once during pairing."
    )
}

/// Log-safe one-line summary of a rejoin attempt: SSID only.
/// The password must never appear in a log line, an error's
/// `detail`, or a command line passed to `netsh`.
#[must_use]
pub fn log_safe_summary(ssid: &str) -> String {
    format!("rejoin: ssid={ssid}")
}

/// Join the PC to the board's Wi-Fi after pairing.
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

/// Windows implementation of [`rejoin_wifi`]: profile import +
/// connect via `netsh`. Never logs the password and never passes
/// it on a command line (it travels only inside the temp XML).
#[cfg(target_os = "windows")]
fn rejoin_wifi_windows(ssid: &str, password: &str) -> Result<(), RejoinError> {
    use std::io::Write as _;

    // DS-45: unique temp path + Drop guard — the old fixed path
    // collided between processes and left the password on disk if
    // the daemon died between write and delete.
    let profile_path = profile_temp_path();
    let xml = build_profile_xml(ssid, password);
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
    // DS-15: each netsh call has a hard 30 s timeout (killed on
    // expiry) instead of the old unbounded `.output()` — and the
    // whole rejoin runs off the manager loop (see main.rs), so a
    // hung Wi-Fi stack stalls nothing.
    let mut add_cmd = std::process::Command::new("netsh");
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
    connect_cmd.args(["wlan", "connect", &format!("name={ssid}")]);
    let connect = run_command_timeout(&mut connect_cmd, NETSH_TIMEOUT, "connect")?;
    if !connect.status.success() {
        return Err(RejoinError::StepFailed {
            step: "connect",
            detail: String::from_utf8_lossy(&connect.stderr).trim().to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_xml_is_wpa2_psk_with_escaped_key() {
        let xml = build_profile_xml("RSH", "p<ass&\"word'");
        assert!(
            xml.contains("<authentication>WPA2PSK</authentication>"),
            "{xml}"
        );
        assert!(xml.contains("<encryption>AES</encryption>"), "{xml}");
        assert!(xml.contains("<name>RSH</name>"), "{xml}");
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
        let summary = log_safe_summary(RSH_SSID);
        assert!(summary.contains("RSH"));
        assert!(!summary.contains("secret"), "{summary}");
    }

    #[test]
    fn manual_instructions_name_the_network_and_tool() {
        let text = manual_instructions(RSH_SSID);
        assert!(text.contains("RSH"), "{text}");
        assert!(text.contains("nmcli"), "{text}");
    }

    #[test]
    fn linux_rejoin_returns_manual_steps_not_silent_failure() {
        // On this Linux host the Windows path cannot run: the
        // helper must say so, with the exact manual command.
        #[cfg(not(target_os = "windows"))]
        {
            let err = rejoin_wifi(RSH_SSID, "one-time-password").unwrap_err();
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
            first
                .file_name()
                .is_some_and(|name| name
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
            other => panic!("expected StepFailed timeout, got {other:?}"),
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
