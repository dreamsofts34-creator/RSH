//! Stub sidecar for Linux tests (`hm-stub-sidecar`).
//!
//! Implements the same JSON-lines protocol as the real C# sidecar
//! (`tools/rsh-hm-sidecar/PROTOCOL.md`) over stdio — minus the DLL and
//! the driver — so the manager and backend can be tested end to end
//! without Windows. Scripted failure modes come from environment
//! variables (all optional; defaults = happy path):
//!
//! | Variable | Effect |
//! |---|---|
//! | `HM_STUB_NO_HELLO=1` | Exit immediately, no hello (EOF before hello). |
//! | `HM_STUB_SILENT=1` | Never answer (spawn/hello timeout path). |
//! | `HM_STUB_BAD_HELLO=1` | First line is garbage, not JSON. |
//! | `HM_STUB_ADMIN=0` | Report `admin: false`. |
//! | `HM_STUB_DRIVER_INSTALLED=1` | Report the driver installed. |
//! | `HM_STUB_WAS_INSTALLED=1` | `install-driver` takes the fast path. |
//! | `HM_STUB_DLL_OK=0` | Report `dll_sha256_ok: false`. |
//! | `HM_STUB_FAIL_CREATE=1` | `create` fails (`HM_STUB_CREATE_ERROR` = code, default `create-failed`). |
//! | `HM_STUB_CRASH_ON_CREATE=1` | Exit(1) when `create` arrives. |
//! | `HM_STUB_CRASH_AFTER_SUBMITS=N` | Exit(1) on the (N+1)-th submit, before replying. |
//! | `HM_STUB_EXIT_AFTER=N` | Exit(0) cleanly after N commands. |
//! | `HM_STUB_LATENCY_US=N` | `submit_us` value in submit replies. |
//! | `HM_STUB_SLOW_MS=N` | Sleep N ms before every reply. |
//! | `HM_STUB_CAPS_JSON={...}` | Capabilities object in `create` replies. |
//!
//! The stub is test tooling, not shipped: it is a binary target of the
//! `rsh-output-hidmaestro` package so integration tests can find it via
//! `env!("CARGO_BIN_EXE_hm-stub-sidecar")`.

use std::env;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

use rsh_output_hidmaestro::{PINNED_DLL_SHA256, PINNED_VERSION};

/// The real custom-profile descriptor (124 B, measured from the v2.1.0
/// DLL in G2) — the stub reports it in `describe-profile` so tests can
/// assert the Rust side's 15-byte expectation against real bytes.
const DESCRIPTOR_HEX: &str = "05010904A10105010930150026FFFF95017510810205010931150026FFFF95017510810205010932150026FFFF95017510810205010933150026FFFF95017510810205010936150026FFFF9501751081020509190129201500250195207501810205010939150025033500460E0166140075089501814245006500C0";

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|v| v != "0" && !v.is_empty())
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn caps_json() -> serde_json::Value {
    if let Ok(raw) = env::var("HM_STUB_CAPS_JSON")
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        return parsed;
    }
    serde_json::json!({
        "has_axis_x": true,
        "has_axis_y": true,
        "has_axis_z": true,
        "has_axis_rx": true,
        "has_axis_slider": true,
        "button_count": 32,
        "has_hat": true
    })
}

// The stub's main loop is one long command match by design (it
// mirrors the real sidecar's loop); splitting it would hurt the
// one-to-one readability with the protocol doc.
#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    if env_flag("HM_STUB_NO_HELLO") {
        return ExitCode::from(0);
    }
    if env_flag("HM_STUB_SILENT") {
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let admin = env::var("HM_STUB_ADMIN").map_or(true, |v| v != "0");
    let driver_installed = env_flag("HM_STUB_DRIVER_INSTALLED");
    let dll_ok = env::var("HM_STUB_DLL_OK").map_or(true, |v| v != "0");
    let slow_ms = env_usize("HM_STUB_SLOW_MS", 0);
    let crash_after_submits = env_usize("HM_STUB_CRASH_AFTER_SUBMITS", usize::MAX);
    let exit_after = env_usize("HM_STUB_EXIT_AFTER", usize::MAX);
    let latency_us = {
        // Env-provided test value (small int); the cast is exact.
        #[allow(clippy::cast_precision_loss)]
        let us = env_usize("HM_STUB_LATENCY_US", 12) as f64;
        us
    };

    let emit = |value: &serde_json::Value, out: &mut dyn Write| {
        if slow_ms > 0 {
            std::thread::sleep(Duration::from_millis(slow_ms as u64));
        }
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    };

    if env_flag("HM_STUB_BAD_HELLO") {
        let _ = writeln!(out, "this is not json");
        let _ = out.flush();
    } else {
        let hello = serde_json::json!({
            "ok": true,
            "hello": "rsh-hm-sidecar",
            "sidecar": "rsh-hm-sidecar-stub",
            "sdk": PINNED_VERSION,
            "protocol": 1,
            "pid": std::process::id(),
            "admin": admin,
            "driver_installed": driver_installed,
            "dll_sha256": PINNED_DLL_SHA256,
            "dll_sha256_ok": dll_ok,
            "transport": "stdio"
        });
        emit(&hello, &mut out);
    }

    let stdin = std::io::stdin();
    let mut created = false;
    let mut submits: usize = 0;
    let mut commands: usize = 0;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        commands += 1;
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) else {
            emit(
                &serde_json::json!({"ok": false, "code": "bad-command", "error": "invalid JSON"}),
                &mut out,
            );
            continue;
        };
        let cmd = parsed.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        match cmd {
            "ping" => {
                emit(
                    &serde_json::json!({"ok": true, "pong": true, "t_us": 123.4}),
                    &mut out,
                );
            }
            "version" => {
                emit(
                    &serde_json::json!({
                        "ok": true, "sdk": PINNED_VERSION,
                        "driver_installed": driver_installed,
                        "admin": admin,
                        "dll_sha256": PINNED_DLL_SHA256,
                        "dll_sha256_ok": dll_ok,
                        "transport": "stdio",
                        "profile": if created { serde_json::Value::String("rsh-wheel-custom".to_string()) } else { serde_json::Value::Null },
                        "transport_index": if created { serde_json::Value::from(0) } else { serde_json::Value::Null }
                    }),
                    &mut out,
                );
            }
            "describe-profile" => {
                emit(
                    &serde_json::json!({
                        "ok": true,
                        "profile": "rsh-wheel-custom",
                        "vid": 0x1209,
                        "pid_value": 0x5253,
                        "vid_pid_provisional": true,
                        "descriptor_len": 124,
                        "descriptor_hex": DESCRIPTOR_HEX,
                        "input_report_size": 15,
                        "input_report_size_legacy_assumption": 26
                    }),
                    &mut out,
                );
            }
            "install-driver" => {
                if env_flag("HM_STUB_WAS_INSTALLED") || driver_installed {
                    emit(
                        &serde_json::json!({"ok": true, "was_installed": true, "now_installed": true, "outcome": "AlreadyCurrent"}),
                        &mut out,
                    );
                } else if !admin {
                    emit(
                        &serde_json::json!({"ok": false, "code": "not-admin", "error": "administrator rights are required to install the driver"}),
                        &mut out,
                    );
                } else {
                    emit(
                        &serde_json::json!({"ok": true, "was_installed": false, "now_installed": true, "outcome": "Installed"}),
                        &mut out,
                    );
                }
            }
            "create" => {
                if env_flag("HM_STUB_CRASH_ON_CREATE") {
                    std::process::exit(1);
                }
                if env_flag("HM_STUB_FAIL_CREATE") {
                    let code = env::var("HM_STUB_CREATE_ERROR")
                        .unwrap_or_else(|_| "create-failed".to_string());
                    emit(
                        &serde_json::json!({"ok": false, "code": code, "error": format!("stub refused create ({code})")}),
                        &mut out,
                    );
                } else {
                    created = true;
                    emit(
                        &serde_json::json!({
                            "ok": true, "created": true, "profile": "rsh-wheel-custom",
                            "transport_index": 0, "driver": PINNED_VERSION,
                            "capabilities": caps_json()
                        }),
                        &mut out,
                    );
                }
            }
            "submit" => {
                submits += 1;
                if submits > crash_after_submits {
                    std::process::exit(1);
                }
                if created {
                    emit(
                        &serde_json::json!({"ok": true, "submit_us": latency_us, "on_submit_latency_us": 9}),
                        &mut out,
                    );
                } else {
                    emit(
                        &serde_json::json!({"ok": false, "code": "no-controller", "error": "no controller"}),
                        &mut out,
                    );
                }
            }
            "submit-raw" => {
                submits += 1;
                if submits > crash_after_submits {
                    std::process::exit(1);
                }
                let hex = parsed.get("report").and_then(|r| r.as_str()).unwrap_or("");
                if hex.len() != 30 {
                    emit(
                        &serde_json::json!({"ok": false, "code": "bad-report-size", "error": format!("expected 15-byte report (30 hex chars), got {} chars", hex.len())}),
                        &mut out,
                    );
                } else if created {
                    emit(
                        &serde_json::json!({"ok": true, "submit_us": latency_us}),
                        &mut out,
                    );
                } else {
                    emit(
                        &serde_json::json!({"ok": false, "code": "no-controller", "error": "no controller"}),
                        &mut out,
                    );
                }
            }
            "neutral" => {
                if created {
                    emit(&serde_json::json!({"ok": true}), &mut out);
                } else {
                    emit(
                        &serde_json::json!({"ok": false, "code": "no-controller", "error": "no controller"}),
                        &mut out,
                    );
                }
            }
            "dispose" => {
                created = false;
                emit(&serde_json::json!({"ok": true, "disposed": true}), &mut out);
            }
            "exit" => {
                emit(&serde_json::json!({"ok": true, "bye": true}), &mut out);
                return ExitCode::from(0);
            }
            _ => {
                emit(
                    &serde_json::json!({"ok": false, "code": "unknown-command", "error": format!("unknown cmd '{cmd}'")}),
                    &mut out,
                );
            }
        }
        if commands >= exit_after {
            return ExitCode::from(0);
        }
    }
    // stdin closed (parent died): clean exit — the real sidecar
    // disposes the device here.
    ExitCode::from(0)
}
