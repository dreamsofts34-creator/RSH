//! JSON-lines protocol between the Rust app and the C# sidecar.
//!
//! One command per line on stdin (or the loopback TCP stream on
//! Windows), one reply per line back. The sidecar processes commands
//! sequentially, so replies arrive in the order commands were sent and
//! the manager holds at most one command in flight — no correlation ids.
//!
//! Wire format (protocol version [`crate::PROTOCOL_VERSION`]):
//! ```text
//! -> {"cmd":"ping"}
//! <- {"ok":true,"pong":true,"t_us":123.4}
//! -> {"cmd":"create","profile":"rsh-wheel-custom"}
//! <- {"ok":true,"created":true,"transport_index":0,"driver":"2.1.0",...}
//! -> {"cmd":"submit","axes":{"X":0.5,"Y":0.0,"Z":0.0,"Rx":0.0,"Slider":0.0},"buttons":0}
//! <- {"ok":true,"submit_us":12.3}
//! <- {"ok":false,"code":"not-admin","error":"..."}
//! ```
//! The full command/reply catalogue lives in
//! `tools/rsh-hm-sidecar/PROTOCOL.md`. The sidecar's first line is
//! always `hello` (validated by [`validate_hello`]); stderr is never
//! part of the protocol (the sidecar logs diagnostics there).

use serde::{Deserialize, Serialize};

use crate::{HidMaestroError, PINNED_DLL_SHA256, PINNED_VERSION, PROTOCOL_VERSION};

/// Axis floats for a `submit` command. Field names are the `HMAxis`
/// names the sidecar parses (`X`/`Y`/`Z`/`Rx`/`Slider`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxesJson {
    /// X — steering (`0.5` = centre).
    #[serde(rename = "X")]
    pub x: f32,
    /// Y — accelerator (`0.0` = released).
    #[serde(rename = "Y")]
    pub y: f32,
    /// Z — brake (`0.0` = released).
    #[serde(rename = "Z")]
    pub z: f32,
    /// Rx — clutch (`0.0` = released).
    #[serde(rename = "Rx")]
    pub rx: f32,
    /// Slider — handbrake (`0.0` = released).
    #[serde(rename = "Slider")]
    pub slider: f32,
}

impl From<crate::GamepadFrame> for AxesJson {
    fn from(frame: crate::GamepadFrame) -> Self {
        Self {
            x: frame.x,
            y: frame.y,
            z: frame.z,
            rx: frame.rx,
            slider: frame.slider,
        }
    }
}

/// A command sent to the sidecar. Serialised with `{"cmd": ...}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    /// Heartbeat / liveness probe. Reply: `{"ok":true,"pong":true,...}`.
    Ping,
    /// Diagnostics. Reply carries SDK/driver versions, admin flag,
    /// DLL-hash verdict, transport index and profile id.
    Version,
    /// Build the custom profile (no driver needed) and report the
    /// descriptor + derived input-report size. Linux-runnable.
    DescribeProfile,
    /// Install the driver when missing (admin). Fast path: already
    /// installed → `AlreadyCurrent`, no install call.
    InstallDriver,
    /// Create the controller for a profile. Only
    /// [`crate::PROFILE_ID`] is accepted by the sidecar.
    Create {
        /// Profile id (`"rsh-wheel-custom"`).
        profile: String,
    },
    /// Submit one frame (`SubmitState` path).
    Submit {
        /// Axis floats.
        axes: AxesJson,
        /// Wheel buttons 1–25 as a bitmask.
        buttons: u32,
    },
    /// Submit one raw report (`SubmitRawReport` path). Hex-encoded bytes;
    /// length must equal the derived report size.
    SubmitRaw {
        /// Hex-encoded report bytes (see [`bytes_to_hex`]).
        bytes: String,
    },
    /// Submit the neutral frame. `raw: true` sends the 15-byte neutral
    /// raw report instead of the `SubmitState` neutral frame.
    Neutral {
        /// True: neutral raw report; false: `SubmitState` neutral.
        raw: bool,
    },
    /// Dispose the controller + context (device removal).
    Dispose,
    /// Clean shutdown: reply `bye`, dispose, exit 0.
    Exit,
}

/// A reply from the sidecar. Every field is optional because replies
/// differ per command; `ok` + `code`/`error` are always present.
/// Unknown fields are ignored (forward-compatible).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Reply {
    /// True on success.
    pub ok: bool,
    /// Machine-readable error code on failure (`"not-admin"`, …).
    pub code: Option<String>,
    /// Human-readable error on failure.
    pub error: Option<String>,
    /// `hello` line marker.
    pub hello: Option<String>,
    /// Sidecar identification (`"rsh-hm-sidecar"`).
    pub sidecar: Option<String>,
    /// Pinned SDK version the sidecar was built against.
    pub sdk: Option<String>,
    /// Protocol version the sidecar speaks.
    pub protocol: Option<u32>,
    /// Sidecar process id.
    pub pid: Option<u32>,
    /// True when the sidecar runs elevated.
    pub admin: Option<bool>,
    /// True when the driver is installed.
    pub driver_installed: Option<bool>,
    /// SHA-256 the sidecar measured for `HIDMaestro.Core.dll`.
    pub dll_sha256: Option<String>,
    /// True when the measured DLL hash matches the pin.
    pub dll_sha256_ok: Option<bool>,
    /// Transport in use (`"stdio"` or `"tcp-loopback"`).
    pub transport: Option<String>,
    /// Loopback token echoed by the sidecar (TCP path only).
    pub token: Option<String>,
    /// Heartbeat marker.
    pub pong: Option<bool>,
    /// Sidecar uptime in microseconds (ping reply).
    pub t_us: Option<f64>,
    /// Submit latency in microseconds (submit reply).
    pub submit_us: Option<f64>,
    /// Last `OnSubmitLatencyMicros` value from the SDK, when wired.
    pub on_submit_latency_us: Option<i64>,
    /// Driver version (`"2.1.0"` form).
    pub driver: Option<String>,
    /// Profile id of the created controller.
    pub profile: Option<String>,
    /// Transport index of the created controller.
    pub transport_index: Option<i32>,
    /// Whether the controller was created by this command.
    pub created: Option<bool>,
    /// `install-driver`: whether the driver was already installed.
    pub was_installed: Option<bool>,
    /// `install-driver`: `InstallDriver()` outcome
    /// (`"AlreadyCurrent"`, …).
    pub outcome: Option<String>,
    /// `install-driver`: whether the driver is installed now.
    pub now_installed: Option<bool>,
    /// Whether the device was disposed by this command.
    pub disposed: Option<bool>,
    /// `exit`: goodbye marker.
    pub bye: Option<bool>,
    /// Custom profile VID (describe-profile / create).
    pub vid: Option<u32>,
    /// Custom profile PID (describe-profile / create).
    pub pid_value: Option<u32>,
    /// Whether the VID/PID is a provisional pid.codes allocation.
    pub vid_pid_provisional: Option<bool>,
    /// Descriptor length in bytes (describe-profile).
    pub descriptor_len: Option<usize>,
    /// Descriptor bytes, lowercase hex (describe-profile).
    pub descriptor_hex: Option<String>,
    /// Input-report size derived from the descriptor, in bytes.
    pub input_report_size: Option<usize>,
    /// The C# series' 26-byte assumption, for comparison (gap record).
    pub input_report_size_legacy_assumption: Option<usize>,
    /// Capabilities the created controller reports.
    pub capabilities: Option<CapabilitiesJson>,
    /// Submit latency sample count (diagnostics).
    pub submit_count: Option<u64>,
}

/// Capabilities as the sidecar reports them after `create` — the REAL
/// sidecar shape (`tools/rsh-hm-sidecar/PROTOCOL.md`, `create` reply):
/// ```json
/// {"axes":5,"buttons":32,"has_hat":true,"input_report_size":15,
///  "submit_state_max_button":25,"submit_state_button_mask":33554431}
/// ```
/// `axes` is a COUNT (1 = X only, 5 = X/Y/Z/Rx/Slider all present);
/// there are no per-axis booleans on the wire. The old shape
/// (`has_axis_x` / … / `button_count`) never existed on the real
/// sidecar — it was a Rust-side invention, and parsing it silently
/// produced `axes: 0` caps for every real `create` reply (C2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CapabilitiesJson {
    /// Axis count (0–5): 1 = X, 2 = +Y, 3 = +Z, 4 = +Rx, 5 = +Slider.
    pub axes: i32,
    /// Button count.
    pub buttons: i32,
    /// Hat present.
    pub has_hat: bool,
    /// Derived input-report size in bytes (15 for `rsh-wheel-custom`).
    pub input_report_size: Option<usize>,
    /// Highest wheel button the `SubmitState` path carries (25).
    pub submit_state_max_button: Option<i32>,
    /// Bitmask for the `SubmitState` buttons (25 bits, `0x01FF_FFFF`).
    pub submit_state_button_mask: Option<u32>,
}

impl CapabilitiesJson {
    /// Derive [`crate::mapping::HidMaestroCapabilities`] from the
    /// wire counts: axis `i` (0-based) is present exactly when
    /// `axes > i` — so `axes == 5` means all five axes are present.
    /// Out-of-range counts clamp (negative → no axes, > 5 → all five);
    /// a `submit_state_max_button` / mask that contradicts the known
    /// `SubmitState` limits (25 / `0x01FF_FFFF`) is ignored — the
    /// derived caps describe the profile, not the sidecar's claim
    /// about the submit path.
    #[must_use]
    pub fn to_capabilities(self) -> crate::mapping::HidMaestroCapabilities {
        let axes = self.axes.clamp(0, 5);
        crate::mapping::HidMaestroCapabilities::new(
            axes >= 1,
            axes >= 2,
            axes >= 3,
            axes >= 4,
            axes >= 5,
            self.buttons,
            self.has_hat,
        )
    }
}

/// Encode bytes as lowercase hex (used for `submit-raw` reports —
/// no extra dependency, and 30 bytes of hex at 200 Hz is negligible).
#[must_use]
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

/// Serialise a command to one JSON line (no trailing newline — the
/// transport adds it).
///
/// # Errors
///
/// Returns [`HidMaestroError::BadReply`] if serialisation fails
/// (unreachable for these types — the error exists so callers do not
/// need to unwrap).
pub fn encode_command(cmd: &Command) -> Result<String, HidMaestroError> {
    serde_json::to_string(cmd).map_err(|e| HidMaestroError::BadReply(e.to_string()))
}

/// Parse one reply line from the sidecar.
///
/// # Errors
///
/// Returns [`HidMaestroError::BadReply`] when the line is not valid
/// JSON or is an empty line.
pub fn parse_reply(line: &str) -> Result<Reply, HidMaestroError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(HidMaestroError::BadReply("empty reply line".to_string()));
    }
    serde_json::from_str::<Reply>(trimmed).map_err(|e| HidMaestroError::BadReply(e.to_string()))
}

/// Validate a `hello` reply: the sidecar must identify itself, speak a
/// protocol version we understand, and prove its DLL hash matches the
/// pin. On the TCP connect-back path the echoed token must match the
/// token we issued.
///
/// STRICT (C-SEC-1 / Security H-2): `ok`, `sdk` and `dll_sha256` are
/// all REQUIRED — a hello that omits any of them is refused, so a
/// compromised sidecar cannot dodge verification by staying silent.
/// `ok` must be `true`; `sdk` must equal the pinned version;
/// `dll_sha256` must equal the pinned DLL hash (case-insensitive);
/// `dll_sha256_ok` must be `true` as well (the measurement is the
/// evidence, the verdict is only the sidecar's claim about it — both
/// must agree with the pin).
///
/// Identity is exact: at least one of `hello` / `sidecar` must be
/// exactly `"rsh-hm-sidecar"` — any other value (including a
/// present-but-wrong `sidecar`) fails. The token echo is compared in
/// constant time.
///
/// # Errors
///
/// Returns the specific validation failure
/// ([`HidMaestroError::BadHello`], `DllHashMismatch`,
/// `VersionMismatch`) — never a bare boolean, so the caller can show
/// the reason.
pub fn validate_hello(reply: &Reply, expected_token: Option<&str>) -> Result<(), HidMaestroError> {
    // C-SEC-1: a hello that reports failure is not a hello at all.
    if !reply.ok {
        return Err(HidMaestroError::BadHello(
            "sidecar hello reported ok:false".to_string(),
        ));
    }
    let identified = reply.hello.as_deref() == Some("rsh-hm-sidecar")
        || reply.sidecar.as_deref() == Some("rsh-hm-sidecar");
    if !identified {
        return Err(HidMaestroError::BadHello(
            "first line is not a sidecar hello".to_string(),
        ));
    }
    if let Some(protocol) = reply.protocol
        && protocol > PROTOCOL_VERSION
    {
        return Err(HidMaestroError::BadHello(format!(
            "sidecar protocol v{protocol} is newer than supported v{PROTOCOL_VERSION} — update RSH"
        )));
    }
    // C-SEC-1: `sdk` is REQUIRED and must equal the pin — a hello
    // without it has not proven which SDK build it runs.
    match reply.sdk.as_deref() {
        Some(sdk) if sdk == PINNED_VERSION => {}
        Some(sdk) => {
            return Err(HidMaestroError::VersionMismatch {
                found: sdk.to_string(),
                pinned: PINNED_VERSION.to_string(),
            });
        }
        None => {
            return Err(HidMaestroError::BadHello(
                "sidecar did not report an SDK version".to_string(),
            ));
        }
    }
    // C-SEC-1: `dll_sha256` is REQUIRED and must match the pin — the
    // measurement is the evidence; a missing measurement is a refusal,
    // not a pass. The verdict (`dll_sha256_ok`) must agree too.
    match reply.dll_sha256.as_deref() {
        Some(measured) if measured.eq_ignore_ascii_case(PINNED_DLL_SHA256) => {}
        Some(measured) => {
            return Err(HidMaestroError::DllHashMismatch {
                expected: PINNED_DLL_SHA256.to_string(),
                actual: measured.to_string(),
            });
        }
        None => {
            return Err(HidMaestroError::BadHello(
                "sidecar did not report a DLL hash measurement".to_string(),
            ));
        }
    }
    match reply.dll_sha256_ok {
        Some(true) => {}
        Some(false) => {
            return Err(HidMaestroError::DllHashMismatch {
                expected: PINNED_DLL_SHA256.to_string(),
                actual: reply.dll_sha256.clone().unwrap_or_else(|| "?".to_string()),
            });
        }
        None => {
            return Err(HidMaestroError::BadHello(
                "sidecar did not report a DLL hash verdict".to_string(),
            ));
        }
    }
    if let Some(expected) = expected_token {
        match reply.token.as_deref() {
            Some(actual) if token_matches(actual, expected) => {}
            Some(_) => {
                return Err(HidMaestroError::BadHello(
                    "sidecar echoed the wrong loopback token — refusing a foreign process"
                        .to_string(),
                ));
            }
            None => {
                return Err(HidMaestroError::BadHello(
                    "sidecar did not echo the loopback token".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Constant-time token comparison: the loopback token is a secret
/// while the sidecar is alive, and a byte-by-byte `==` leaks the
/// matching prefix length through timing. Lengths differ → not equal
/// (the token is fixed-length 32 hex chars, so a length mismatch is
/// itself a mismatch, not a leak of anything secret).
fn token_matches(actual: &str, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    bool::from(actual.as_bytes().ct_eq(expected.as_bytes()))
}

/// Map a failed reply (`ok: false`) onto the matching error.
///
/// # Errors
///
/// Always returns an error — this is a constructor for the failure
/// case, so the return type is `Result<(), HidMaestroError>` for
/// ergonomic `?` use at call sites.
pub fn reply_to_error(reply: &Reply) -> Result<(), HidMaestroError> {
    if reply.ok {
        return Ok(());
    }
    let msg = reply
        .error
        .clone()
        .unwrap_or_else(|| "sidecar reported failure".to_string());
    let code = reply.code.as_deref().unwrap_or("");
    Err(match code {
        "not-admin" => HidMaestroError::NotAdmin,
        "driver-not-installed" => HidMaestroError::DriverNotInstalled(msg),
        "driver-install-failed" => HidMaestroError::DriverInstallFailed(msg),
        "dll-hash-mismatch" => HidMaestroError::DllHashMismatch {
            expected: PINNED_DLL_SHA256.to_string(),
            actual: msg,
        },
        "profile-refused" => HidMaestroError::ProfileRefused(msg),
        "create-failed" => HidMaestroError::CreateFailed(msg),
        "no-controller" | "bad-command" | "unknown-command" | "bad-report-size" => {
            HidMaestroError::BadReply(format!("{code}: {msg}"))
        }
        _ => HidMaestroError::BadReply(msg),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_ping_is_one_json_line() {
        let line = encode_command(&Command::Ping).unwrap();
        assert_eq!(line, r#"{"cmd":"ping"}"#);
        assert!(!line.contains('\n'));
    }

    #[test]
    fn encode_create_uses_kebab_case() {
        let line = encode_command(&Command::Create {
            profile: "rsh-wheel-custom".to_string(),
        })
        .unwrap();
        assert_eq!(line, r#"{"cmd":"create","profile":"rsh-wheel-custom"}"#);
    }

    #[test]
    fn encode_submit_axes_use_axis_names() {
        let line = encode_command(&Command::Submit {
            axes: AxesJson {
                x: 0.5,
                y: 0.0,
                z: 0.0,
                rx: 0.0,
                slider: 0.0,
            },
            buttons: 5,
        })
        .unwrap();
        assert!(line.contains(r#""X":0.5"#));
        assert!(line.contains(r#""Rx":0.0"#));
        assert!(line.contains(r#""Slider":0.0"#));
        assert!(line.contains(r#""buttons":5"#));
    }

    #[test]
    fn encode_submit_raw_and_exit() {
        // C1: the field is `bytes` (what the real sidecar reads) —
        // `report` is rejected by the sidecar.
        assert_eq!(
            encode_command(&Command::SubmitRaw {
                bytes: "AAEC".to_string()
            })
            .unwrap(),
            r#"{"cmd":"submit-raw","bytes":"AAEC"}"#
        );
        assert_eq!(encode_command(&Command::Exit).unwrap(), r#"{"cmd":"exit"}"#);
        assert_eq!(
            encode_command(&Command::InstallDriver).unwrap(),
            r#"{"cmd":"install-driver"}"#
        );
    }

    #[test]
    fn encode_neutral_carries_raw_flag() {
        // L2: Rust sends `neutral` (watchdog / shutdown) with the raw
        // flag, so the sidecar knows which neutral frame to send.
        assert_eq!(
            encode_command(&Command::Neutral { raw: false }).unwrap(),
            r#"{"cmd":"neutral","raw":false}"#
        );
        assert_eq!(
            encode_command(&Command::Neutral { raw: true }).unwrap(),
            r#"{"cmd":"neutral","raw":true}"#
        );
    }

    #[test]
    fn parse_reply_ok_submit() {
        let reply = parse_reply(r#"{"ok":true,"submit_us":12.3}"#).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.submit_us, Some(12.3));
        assert_eq!(reply.code, None);
    }

    #[test]
    fn parse_reply_error_with_code() {
        let reply =
            parse_reply(r#"{"ok":false,"code":"not-admin","error":"admin needed"}"#).unwrap();
        assert!(!reply.ok);
        assert_eq!(reply.code.as_deref(), Some("not-admin"));
        assert_eq!(reply.error.as_deref(), Some("admin needed"));
    }

    #[test]
    fn parse_reply_rejects_empty_and_garbage() {
        assert!(parse_reply("").is_err());
        assert!(parse_reply("   ").is_err());
        assert!(parse_reply("not json").is_err());
        assert!(parse_reply("{").is_err());
    }

    #[test]
    fn parse_reply_ignores_unknown_fields() {
        let reply = parse_reply(r#"{"ok":true,"future_field":123,"pong":true}"#).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.pong, Some(true));
    }

    #[test]
    fn validate_hello_good_passes() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true,"admin":true}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, None).is_ok());
    }

    #[test]
    fn validate_hello_wrong_sidecar_fails() {
        let reply = parse_reply(r#"{"ok":true,"hello":"something-else"}"#).unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// H1: a present-but-wrong `sidecar` field no longer counts as
    /// identification — the old check passed any non-`None` value.
    #[test]
    fn validate_hello_wrong_sidecar_field_value_fails() {
        let reply =
            parse_reply(r#"{"ok":true,"sidecar":"something-else","dll_sha256_ok":true}"#).unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// H1: the `sidecar` field alone identifies, but only with the
    /// exact value — and the strict hello still needs sdk + DLL hash.
    #[test]
    fn validate_hello_sidecar_field_alone_identifies() {
        let reply = parse_reply(
            r#"{"ok":true,"sidecar":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, None).is_ok());
    }

    /// H1: a hello with no DLL-hash verdict at all is refused — a
    /// sidecar must not dodge verification by staying silent.
    #[test]
    fn validate_hello_missing_dll_verdict_fails() {
        let reply = parse_reply(r#"{"ok":true,"hello":"rsh-hm-sidecar"}"#).unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// H1: a measured hash that contradicts the pin fails even when
    /// the sidecar claims `ok: true` — the measurement is the
    /// evidence, the verdict only its claim about it.
    #[test]
    fn validate_hello_dll_hash_contradicts_verdict_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256_ok":true,"dll_sha256":"00"}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::DllHashMismatch { .. })
        ));
    }

    #[test]
    fn validate_hello_newer_protocol_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","protocol":99,"dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, None).is_err());
    }

    #[test]
    fn validate_hello_older_protocol_passes() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","protocol":1,"sdk":"2.1.0","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, None).is_ok());
    }

    #[test]
    fn validate_hello_bad_dll_hash_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256_ok":false,"dll_sha256":"00"}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::DllHashMismatch { .. })
        ));
    }

    #[test]
    fn validate_hello_wrong_sdk_version_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.0.0","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn validate_hello_token_checked_on_tcp_path() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true,"token":"abc"}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, Some("abc")).is_ok());
        assert!(validate_hello(&reply, Some("xyz")).is_err());
        assert!(validate_hello(&reply, None).is_ok());
    }

    #[test]
    fn validate_hello_missing_token_on_tcp_path_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(validate_hello(&reply, Some("abc")).is_err());
    }

    /// C-SEC-1 / H-2: `ok: false` is refused even when every other
    /// field is perfect — a hello that reports failure is not a hello.
    #[test]
    fn validate_hello_ok_false_fails() {
        let reply = parse_reply(
            r#"{"ok":false,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// C-SEC-1 / H-2: a missing `sdk` is refused — the sidecar must
    /// prove which SDK build it runs, not stay silent about it.
    #[test]
    fn validate_hello_missing_sdk_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// C-SEC-1 / H-2: a missing `dll_sha256` measurement is refused
    /// even when the sidecar claims `dll_sha256_ok: true` — the
    /// verdict without a measurement is not evidence.
    #[test]
    fn validate_hello_missing_dll_measurement_fails() {
        let reply = parse_reply(
            r#"{"ok":true,"hello":"rsh-hm-sidecar","sdk":"2.1.0","dll_sha256_ok":true}"#,
        )
        .unwrap();
        assert!(matches!(
            validate_hello(&reply, None),
            Err(HidMaestroError::BadHello(_))
        ));
    }

    /// C2: the verbatim real-sidecar `create` capabilities shape
    /// (PROTOCOL.md) parses, and `axes == 5` derives all five axes.
    #[test]
    fn capabilities_real_sidecar_shape_parses() {
        let reply = parse_reply(
            r#"{"ok":true,"created":true,"capabilities":{"axes":5,"buttons":32,"has_hat":true,"input_report_size":15,"submit_state_max_button":25,"submit_state_button_mask":33554431}}"#,
        )
        .unwrap();
        let caps = reply.capabilities.expect("capabilities present");
        assert_eq!(caps.axes, 5);
        assert_eq!(caps.buttons, 32);
        assert!(caps.has_hat);
        assert_eq!(caps.input_report_size, Some(15));
        assert_eq!(caps.submit_state_max_button, Some(25));
        assert_eq!(caps.submit_state_button_mask, Some(0x01FF_FFFF));
        assert_eq!(
            caps.to_capabilities(),
            crate::mapping::HidMaestroCapabilities::custom_wheel()
        );
    }

    /// C2: axis counts derive per-axis presence (`axes == 1` is X
    /// only; `axes == 3` is X/Y/Z); the old per-axis-boolean shape is
    /// gone — an old-shape object parses as zero axes, never as a
    /// full wheel.
    #[test]
    fn capabilities_axis_count_derives_axes() {
        let one =
            parse_reply(r#"{"ok":true,"capabilities":{"axes":1,"buttons":0,"has_hat":false}}"#)
                .unwrap()
                .capabilities
                .unwrap()
                .to_capabilities();
        assert_eq!(
            one,
            crate::mapping::HidMaestroCapabilities::new(true, false, false, false, false, 0, false)
        );
        let three =
            parse_reply(r#"{"ok":true,"capabilities":{"axes":3,"buttons":8,"has_hat":true}}"#)
                .unwrap()
                .capabilities
                .unwrap()
                .to_capabilities();
        assert!(three.has_axis_x && three.has_axis_y && three.has_axis_z);
        assert!(!three.has_axis_rx && !three.has_axis_slider);
        assert_eq!(three.button_count, 8);
        assert!(three.has_hat);
        // Old shape (never sent by the real sidecar): no `axes` field
        // → 0 axes, so a stale fixture can never masquerade as a wheel.
        let stale = parse_reply(
            r#"{"ok":true,"capabilities":{"has_axis_x":true,"has_axis_y":true,"has_axis_z":true,"has_axis_rx":true,"has_axis_slider":true,"button_count":32,"has_hat":true}}"#,
        )
        .unwrap()
        .capabilities
        .unwrap()
        .to_capabilities();
        assert!(!stale.has_axis_x);
        assert_eq!(stale.button_count, 0);
    }

    #[test]
    fn reply_to_error_maps_known_codes() {
        let reply = parse_reply(r#"{"ok":false,"code":"not-admin","error":"x"}"#).unwrap();
        assert!(matches!(
            reply_to_error(&reply),
            Err(HidMaestroError::NotAdmin)
        ));
        let reply =
            parse_reply(r#"{"ok":false,"code":"profile-refused","error":"usbip"}"#).unwrap();
        assert!(matches!(
            reply_to_error(&reply),
            Err(HidMaestroError::ProfileRefused(_))
        ));
        let reply = parse_reply(r#"{"ok":false,"code":"create-failed","error":"x"}"#).unwrap();
        assert!(matches!(
            reply_to_error(&reply),
            Err(HidMaestroError::CreateFailed(_))
        ));
        let reply =
            parse_reply(r#"{"ok":false,"code":"driver-install-failed","error":"x"}"#).unwrap();
        assert!(matches!(
            reply_to_error(&reply),
            Err(HidMaestroError::DriverInstallFailed(_))
        ));
    }

    #[test]
    fn reply_to_error_ok_reply_is_ok() {
        let reply = parse_reply(r#"{"ok":true}"#).unwrap();
        assert!(reply_to_error(&reply).is_ok());
    }

    #[test]
    fn reply_to_error_unknown_code_is_bad_reply() {
        let reply = parse_reply(r#"{"ok":false,"code":"weird","error":"boom"}"#).unwrap();
        assert!(matches!(
            reply_to_error(&reply),
            Err(HidMaestroError::BadReply(_))
        ));
    }
}
