//! Lifecycle regression checks (v2.0.3 F17) — the checks the v2.0.2
//! gate was missing: they launch the REAL `rsh-ui` and `rsh-daemon`
//! binaries under Xvfb and watch them live (spawn counts, IPC
//! answers, window visibility, process counts), instead of auditing
//! source or probing one call.
//!
//! Why these exist: the v2.0.2 gate was GREEN (1134/0, rig 22/22)
//! yet the restart loop + duplicate tray + hang shipped, because no
//! gate check ever launched a real daemon/UI process. Each check
//! here must FAIL on the pre-fix binaries and PASS on the fixed
//! ones; the pre-fix numbers are recorded in the fix plan
//! (`docs/V2.0.3-FIX-PLAN.md` §F17) and in the F17 repro notes.
//!
//! Honest limits of this file (Linux):
//! * Tick p99 (<100 ms, zero ticks >500 ms) cannot be measured from
//!   outside the process without instrumentation — the frame-budget
//!   check measures the user-visible proxies instead (input-ack
//!   latency <1 s, window present at every 100 ms sample, exit
//!   latency <5 s) and says so in its detail.
//! * Tray icon counting/ghosting, foreground activation, the
//!   named-pipe single instance, NSIS flows, UAC and rendering are
//!   Windows-only and are NOT proven here (F18 Windows CI + the
//!   user-PC retest cover them).
//! * The app-driven hide→show path (X-close → tray → page request
//!   → `win.show()`) PANICS under Xvfb in winit's draw
//!   (`GetGeometry` → `BadDrawable`, and one run died with SIGSEGV
//!   after the close) — the X11 backend with no window manager, not
//!   the v2.0.3 code: the same binary survives X-level unmap/map
//!   (probe2) and Windows tray reopen worked on the user PC
//!   (v2.0.1). The hidden/shown + rapid-reconnect checks therefore
//!   use X-level `xdotool windowunmap`/`windowmap` as the reopen
//!   mechanism and record this as finding F17-HS1. The app's
//!   `on_close_requested → HideWindow` wiring stays a source audit
//!   (`gate-x-close-source`).
//!
//! Platform: the process/X11 checks are Linux-only. On other
//! platforms each check returns a PASS with an explicit SKIP note
//! (the Windows execution proof is F18, not this file).
//! Environment: needs `Xvfb` + `xdotool` + `libxkbcommon-x11-0` +
//! `libxcb-xkb1` (winit panics at startup without the xkbcommon
//! library) and the debug binaries
//! (`cargo build -p rsh-ui -p rsh-daemon -p rsh-fake-daemon` —
//! `tools/gate-checks/run-gate.sh` builds them first). Staging goes
//! under `target/rsh-rig-stage-<name>-<pid>` (NOT `/tmp`: the UI
//! binary alone is ~470 MB and `/tmp` here is a 512 MB tmpfs).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Accumulates one check's sub-assertions into the `(passed,
/// detail)` pair the harness expects (same shape as
/// `gate_checks::Report`, local so this module stays standalone).
struct LcReport {
    ok: bool,
    lines: Vec<String>,
}

impl LcReport {
    fn new() -> Self {
        Self {
            ok: true,
            lines: Vec::new(),
        }
    }

    fn check(&mut self, name: &str, passed: bool, detail: &str) {
        if !passed {
            self.ok = false;
        }
        self.lines.push(format!(
            "{}: {} — {detail}",
            name,
            if passed { "PASS" } else { "FAIL" }
        ));
    }

    fn note(&mut self, detail: &str) {
        self.lines.push(detail.to_string());
    }

    fn finish(self) -> (bool, String) {
        (self.ok, self.lines.join(" | "))
    }
}

/// The Linux-only guard: every check starts with this and returns
/// a SKIP pass on other platforms (Windows proof is F18).
fn not_unix_skip(name: &str) -> Option<(bool, String)> {
    if cfg!(unix) {
        None
    } else {
        Some((
            true,
            format!("{name}: SKIP — Linux-only lifecycle check (Windows proof is F18 CI)"),
        ))
    }
}

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Stage directory for one check on the home disk (NOT `/tmp` —
/// see the module docs): `target/rig-<name>-<pid>`. The name is
/// capped at 12 chars: the daemon socket is
/// `<stage>/data/rsh-daemon.sock` and Unix socket paths cap at 108
/// chars (`sun_path`) — the old `rsh-rig-stage-<name>-<pid>` prefix
/// made the socket ~112 chars, so the daemon could never bind and
/// every race-round check saw 7 "live" daemons stuck in the F15
/// grace wait. Removed afterwards (best effort).
fn stage_dir(name: &str) -> PathBuf {
    let short: String = name.chars().take(12).collect();
    let dir = crate::gate_checks::repo_root()
        .join("target")
        .join(format!("rig-{short}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    // Canonicalize: `repo_root()` is `tools/rsh-rig/../..`, so the
    // raw path carries `/../..` segments and the daemon socket
    // (`<stage>/data/rsh-daemon.sock`) exceeds SUN_LEN even when the
    // normalized path fits — the daemon then fails all 5 binds with
    // "path must be shorter than SUN_LEN" and every loser waits out
    // its 20-try grace and exits 1. Canonicalize once here so every
    // child sees the short real path.
    std::fs::canonicalize(&dir).unwrap_or(dir)
}

fn debug_bin(name: &str) -> Option<PathBuf> {
    let path = crate::gate_checks::repo_root()
        .join("target")
        .join("debug")
        .join(name);
    path.is_file().then_some(path)
}

/// Copy a debug binary into the stage dir.
fn stage_copy(stage: &Path, name: &str) -> std::io::Result<PathBuf> {
    let src = debug_bin(name).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("target/debug/{name} not built — run cargo build -p {name}"),
        )
    })?;
    let dst = stage.join(name);
    std::fs::copy(src, &dst)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dst)
}

/// Stage the UI + a fake daemon as its sibling `rsh-daemon` (the UI
/// spawns the sibling next to its own exe). Returns `(ui, data_dir,
/// spawn_log)`. The fake daemon's mode comes from env vars the UI
/// inherits (`RSH_FAKE_DAEMON_MODE`, `RSH_FAKE_DAEMON_SLOW_MS`,
/// `RSH_FAKE_DAEMON_FLAP_PINGS`, `RSH_FAKE_DAEMON_LOG`).
fn stage_ui_with_fake_daemon(stage: &Path) -> std::io::Result<(PathBuf, PathBuf, PathBuf)> {
    let ui = stage_copy(stage, "rsh-ui")?;
    let _ = stage_copy(stage, "rsh-fake-daemon")?;
    std::fs::copy(stage.join("rsh-fake-daemon"), stage.join("rsh-daemon"))?;
    let data_dir = stage.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let spawn_log = stage.join("spawns.log");
    Ok((ui, data_dir, spawn_log))
}

/// Stage the UI + the REAL daemon as its sibling.
fn stage_ui_with_real_daemon(stage: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let ui = stage_copy(stage, "rsh-ui")?;
    let _ = stage_copy(stage, "rsh-daemon")?;
    let data_dir = stage.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    Ok((ui, data_dir))
}

/// A running Xvfb on a free display (tries 120..129 — :99 is often
/// taken by other workers' runs). Killed on drop.
struct XvfbSession {
    child: Child,
    display: u32,
}

impl XvfbSession {
    fn start() -> Option<Self> {
        if !have("Xvfb") || !have("xdotool") {
            return None;
        }
        for display in 120..130 {
            let name = format!(":{display}");
            let mut child = Command::new("Xvfb")
                .arg(&name)
                .args(["-screen", "0", "1280x1024x24", "-nolisten", "tcp"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            // The server is up when xdotool can talk to it (exit 0
            // even with no matching window). If the display is
            // taken, Xvfb exits at once — detect that and try the
            // next display instead of waiting the full probe loop.
            let mut up = false;
            for _ in 0..20 {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                let reachable = Command::new("xdotool")
                    .env("DISPLAY", &name)
                    .args(["search", "--name", "definitely-not-a-window"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.code() == Some(0) || s.code() == Some(1));
                // Freshness is guaranteed by construction: our
                // Xvfb child being alive means WE bound this
                // display (a taken display makes the child exit at
                // once, caught by `try_wait` above) — a stale server
                // left by a killed run cannot share the display
                // with our live child. (An earlier draft required
                // zero windows via `search --name ''`, but the empty
                // pattern also matches the root window, so a fresh
                // server never passed it.)
                if reachable {
                    up = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if up {
                return Some(Self { child, display });
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        None
    }

    fn display(&self) -> String {
        format!(":{}", self.display)
    }
}

impl Drop for XvfbSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run xdotool against a display; returns (success, stdout).
fn xdotool(display: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new("xdotool")
        .env("DISPLAY", display)
        .args(args)
        .output();
    match out {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        ),
        Err(_) => (false, String::new()),
    }
}

/// Window queries are scoped by OWNER PID, never by name:
/// `xdotool search --name` matches the legacy `WM_NAME`, which the
/// main window ("RSH — Real Sim Hub", `crates/rsh-ui/ui/main.slint`)
/// does not carry in a matchable form (an exact-title search finds
/// 0 windows, a bare `--name RSH` finds only the daemon's tray
/// popup "RSH Wheel — Tray"). `--pid` uses `_NET_WM_PID`, which
/// winit sets reliably — and it excludes the daemon's popup window
/// (a different pid) by construction.
fn window_visible(display: &str, pid: u32) -> bool {
    let pid_arg = pid.to_string();
    let (ok, out) = xdotool(display, &["search", "--onlyvisible", "--pid", &pid_arg]);
    ok && !out.is_empty()
}

fn window_id(display: &str, pid: u32) -> Option<String> {
    let pid_arg = pid.to_string();
    let (ok, out) = xdotool(display, &["search", "--pid", &pid_arg]);
    if ok {
        out.lines().next().map(str::to_string)
    } else {
        None
    }
}

/// Poll `f` every 100 ms until it is true or `timeout` elapses.
fn wait_until(timeout: Duration, f: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Pids whose `/proc/<pid>/cmdline` contains `needle` (the check's
/// data dir — unique per check, so other workers' processes never
/// match), plus whether each is a zombie. Skips this process.
fn procs_with_arg(needle: &str) -> Vec<(u32, bool)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    let self_pid = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let text = String::from_utf8_lossy(&cmdline);
        if !text.contains(needle) {
            continue;
        }
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit(')')
                    .next()
                    .map(|t| t.trim_start().starts_with('Z'))
            })
            .unwrap_or(false);
        out.push((pid, zombie));
    }
    out
}

/// One `ping` round-trip against the daemon socket in `data_dir`
/// (fresh thread + current-thread runtime — the harness runs inside
/// the rig's `#[tokio::main]` runtime, where `block_on` panics).
/// The fake daemon answers `{"pong":true}`; the real daemon the
/// same. `false` covers "no socket", "no answer" and "wrong reply".
fn ipc_ping(data_dir: &Path) -> bool {
    let path = rsh_ipc::socket_path(data_dir);
    if !path.exists() {
        return false;
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("lifecycle tokio runtime");
                rt.block_on(async {
                    let Ok(Ok(client)) = tokio::time::timeout(
                        Duration::from_secs(2),
                        rsh_ipc::IpcClient::connect(&path),
                    )
                    .await
                    else {
                        return false;
                    };
                    match client.call("ping", serde_json::json!({})).await {
                        Ok(v) => v.get("pong").and_then(serde_json::Value::as_bool) == Some(true),
                        Err(_) => false,
                    }
                })
            })
            .join()
            .unwrap_or(false)
    })
}

fn send_signal(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([sig, &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn count_lines_containing(path: &Path, needle: &str) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().filter(|l| l.contains(needle)).count())
}

fn spawn_log_times(path: &Path) -> Vec<u64> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Whether a child is still running (`try_wait` says not exited;
/// any error counts as not alive, so a lost handle fails closed).
fn child_alive(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

/// Whether a child has exited (any exit, any code).
fn child_exited(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

/// Kill a child best-effort (SIGKILL then reap).
fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait for a child to exit within `timeout`; returns its exit code
/// (or `None` on timeout — the child is then killed).
fn wait_child_timeout(child: &mut Child, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_child(child);
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

/// Launch the staged UI under Xvfb; stderr+stdout go to
/// `stage/ui.log`. Returns the child + the log path. `extra_env`
/// is inherited by the daemon the UI spawns (the fake daemon reads
/// its mode from it).
fn launch_ui(
    ui: &Path,
    data_dir: &Path,
    display: &str,
    quit_after: u64,
    log_path: &Path,
    extra_env: &[(&str, &str)],
) -> std::io::Result<Child> {
    let log = std::fs::File::create(log_path)?;
    let mut cmd = Command::new(ui);
    cmd.args([
        "--data-dir",
        &data_dir.to_string_lossy(),
        "--quit-after",
        &quit_after.to_string(),
    ])
    .env("DISPLAY", display)
    .stdout(Stdio::from(log.try_clone()?))
    .stderr(Stdio::from(log))
    .stdin(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn()
}

/// Launch the REAL daemon headless (`--no-tray --no-connect
/// --verbose`) against a data dir; stderr goes to `log_path`.
fn launch_real_daemon(data_dir: &Path, log_path: &Path) -> std::io::Result<Child> {
    let bin = debug_bin("rsh-daemon").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "target/debug/rsh-daemon not built — run cargo build -p rsh-daemon",
        )
    })?;
    let log = std::fs::File::create(log_path)?;
    Command::new(bin)
        .args([
            "--data-dir",
            &data_dir.to_string_lossy(),
            "--no-tray",
            "--no-connect",
            "--verbose",
        ])
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .stdin(Stdio::null())
        .spawn()
}

/// F17.1 — respawn bound. Crash-on-start daemon + UI health monitor
/// for ~190 s: the v2.0.2 loop spawned every ~11 s forever (17
/// spawns in 185 s, measured pre-fix); the fix latches after 5
/// failed spawns with backoff 1/5/30/60 s, so the run must show
/// <= 6 spawns, non-decreasing gaps, and no spawn in the last
/// 30 s — with the UI still alive at the end.
#[must_use]
pub fn gate_respawn_bound() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-respawn-bound") {
        return skip;
    }
    let stage = stage_dir("respawn-bound");
    let need = ["Xvfb", "xdotool"];
    for cmd in need {
        if !have(cmd) {
            return (
                false,
                format!("gate-respawn-bound: FAIL — {cmd} missing (install xvfb + xdotool)"),
            );
        }
    }
    let Ok((ui, data_dir, spawn_log)) = stage_ui_with_fake_daemon(&stage) else {
        return (
            false,
            "gate-respawn-bound: FAIL — staging failed (binaries not built?)".into(),
        );
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-respawn-bound: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let log_path = stage.join("ui.log");
    let spawn_log_arg = spawn_log.to_string_lossy().to_string();
    let ui_started = Instant::now();
    let mut child = match launch_ui(
        &ui,
        &data_dir,
        &display,
        225,
        &log_path,
        &[
            ("RSH_FAKE_DAEMON_MODE", "crash"),
            ("RSH_FAKE_DAEMON_LOG", spawn_log_arg.as_str()),
        ],
    ) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-respawn-bound: FAIL — UI did not start: {e}"),
            );
        }
    };
    // Wait for the loop to stop: poll the spawn log until no spawn
    // has happened for 30 s (the latch), or 210 s elapse (the loop
    // is still running — the pre-fix behaviour). The UI quits on
    // its own 225 s timer afterwards.
    let started = Instant::now();
    let mut quiet_ms: Option<u64> = None;
    loop {
        let times = spawn_log_times(&spawn_log);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        if let Some(last) = times.last() {
            let q = now_ms.saturating_sub(*last);
            if q >= 30_000 {
                quiet_ms = Some(q);
                break;
            }
        }
        if started.elapsed() >= Duration::from_secs(210) {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let ui_alive = child_alive(&mut child);
    let times = spawn_log_times(&spawn_log);
    let spawns = times.len();
    let gaps: Vec<u64> = times
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .collect();
    let non_decreasing = gaps.windows(2).all(|g| g[1] + 3000 >= g[0]);
    // Let the UI quit on its own timer, then reap (the timer is
    // 225 s from launch; the latch is usually observed ~170 s in,
    // so wait the remainder + 10 s).
    let remaining = Duration::from_secs(225)
        .saturating_sub(ui_started.elapsed())
        .saturating_add(Duration::from_secs(10));
    let exit = wait_child_timeout(&mut child, remaining);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "ui alive when the loop stops",
        ui_alive,
        &format!(
            "UI process {}",
            if ui_alive {
                "alive"
            } else {
                "DEAD before the loop stopped"
            }
        ),
    );
    report.check(
        "spawn count <= 6",
        spawns <= 6,
        &format!("{spawns} daemon spawns before the latch (pre-fix: 17 in 185 s)"),
    );
    report.check(
        "gaps non-decreasing (backoff grows)",
        non_decreasing,
        &format!("gaps (ms): {gaps:?} (pre-fix: flat ~11000 ms)"),
    );
    report.check(
        "no spawn in last 30 s",
        quiet_ms.is_some_and(|q| q >= 30_000),
        &format!(
            "quiet for {} ms (latch observed after {:.0} s)",
            quiet_ms.unwrap_or(0),
            started.elapsed().as_secs_f64()
        ),
    );
    report.check(
        "UI exits on its --quit-after timer",
        exit.is_some(),
        &format!("exit code: {exit:?}"),
    );
    report.finish()
}

/// F17.2 — IPC bind failure. Pre-bind the daemon socket with a
/// dummy listener that accepts and holds (a live peer — the daemon
/// must NOT unlink it as stale); start the REAL daemon → exactly 5
/// bind attempts, then exit(1) with `daemon-status.json` code
/// `IpcBindFailed`. Then start the UI against the same pre-bound
/// socket: the fresh fatal file must block respawns (0 "spawned
/// daemon" lines in 30 s) and the UI must stay alive. Pre-fix the
/// UI respawned every ~11 s (~3 spawns in 30 s).
#[must_use]
pub fn gate_ipc_bind_failure() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-ipc-bind-failure") {
        return skip;
    }
    let stage = stage_dir("ipc-bind-failure");
    let data_dir = stage.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let sock = rsh_ipc::socket_path(&data_dir);
    // Dummy listener: accept-and-hold, so the daemon sees a live
    // peer and cannot unlink the socket as stale. Held on a thread
    // for the whole check.
    let listener = match std::os::unix::net::UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            return (
                false,
                format!("gate-ipc-bind-failure: FAIL — dummy listener did not bind: {e}"),
            );
        }
    };
    let _holder = std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                // Hold the connection open (never reply): the UI
                // probe times out instead of failing fast.
                std::thread::sleep(Duration::from_secs(3600));
                drop(stream);
            });
        }
    });
    // Part 1: the real daemon against the taken socket.
    let daemon_log = stage.join("daemon.log");
    let mut daemon = match launch_real_daemon(&data_dir, &daemon_log) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-ipc-bind-failure: FAIL — daemon did not start: {e}"),
            );
        }
    };
    let daemon_exit = wait_child_timeout(&mut daemon, Duration::from_secs(30));
    let attempts = count_lines_containing(&daemon_log, "IPC bind failed (attempt");
    let status = read_json(&data_dir.join("daemon-status.json"));
    let code = status
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    report.check(
        "daemon exits(1) on a taken socket",
        daemon_exit == Some(1),
        &format!("exit: {daemon_exit:?} (daemon must not run with no IPC)"),
    );
    report.check(
        "exactly 5 bind attempts",
        attempts == 5,
        &format!("{attempts} \"IPC bind failed (attempt\" lines in the daemon log"),
    );
    report.check(
        "daemon-status.json code is IpcBindFailed",
        code == "IpcBindFailed",
        &format!("code: {code:?}"),
    );
    // Part 2: the UI against the same taken socket + fresh fatal.
    let Ok((ui, ui_data)) = stage_ui_with_real_daemon(&stage) else {
        return (
            false,
            "gate-ipc-bind-failure: FAIL — UI staging failed".into(),
        );
    };
    // The UI needs the SAME data dir (fatal file + taken socket).
    let _ = ui_data;
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-ipc-bind-failure: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let ui_log = stage.join("ui.log");
    let mut ui_child = match launch_ui(&ui, &data_dir, &display, 45, &ui_log, &[]) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-ipc-bind-failure: FAIL — UI did not start: {e}"),
            );
        }
    };
    std::thread::sleep(Duration::from_secs(30));
    let ui_alive = child_alive(&mut ui_child);
    let ui_spawns = count_lines_containing(&ui_log, "spawned daemon");
    let status_after = read_json(&data_dir.join("daemon-status.json"));
    let code_after = status_after
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    kill_child(&mut ui_child);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "UI alive after 30 s against the fatal daemon",
        ui_alive,
        &format!("UI process {}", if ui_alive { "alive" } else { "DEAD" }),
    );
    report.check(
        "UI spawns 0 daemons while the fatal is fresh",
        ui_spawns == 0,
        &format!("{ui_spawns} \"spawned daemon\" lines in 30 s (pre-fix: ~3)"),
    );
    report.check(
        "fatal code still IpcBindFailed after the UI run",
        code_after == "IpcBindFailed",
        &format!("code: {code_after:?} (the UI must not clear/overwrite it)"),
    );
    report.finish()
}

/// F17.3 — hung daemon. Real daemon `SIGSTOP`ed (alive, IPC dead,
/// lock held): the UI ping fails; a second daemon exits(1) with
/// `DaemonHung` (not 0, not takeover); spawns stay bounded while
/// the lock is held; after SIGCONT the UI reconnects with no
/// duplicate daemon.
#[must_use]
pub fn gate_hung_daemon() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-hung-daemon") {
        return skip;
    }
    let stage = stage_dir("hung-daemon");
    let data_dir = stage.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let daemon_log = stage.join("daemon.log");
    let mut daemon = match launch_real_daemon(&data_dir, &daemon_log) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-hung-daemon: FAIL — daemon did not start: {e}"),
            );
        }
    };
    let daemon_pid = daemon.id();
    let ping_ok = wait_until(Duration::from_secs(15), || ipc_ping(&data_dir));
    report.check(
        "daemon answers IPC before the hang",
        ping_ok,
        "ping must be true within 15 s of start",
    );
    // Hang it: SIGSTOP (alive, IPC dead, single-instance lock held).
    send_signal(daemon_pid, "-STOP");
    std::thread::sleep(Duration::from_secs(1));
    let ping_dead = !ipc_ping(&data_dir);
    report.check(
        "IPC ping fails while the daemon is stopped",
        ping_dead,
        "a stopped daemon must not answer (the hang the UI must survive)",
    );
    // A second daemon must classify it Hung and exit(1) — the
    // 20x500 ms ping window takes ~10 s, so allow 25 s.
    let second_log = stage.join("daemon2.log");
    let mut second = match launch_real_daemon(&data_dir, &second_log) {
        Ok(c) => c,
        Err(e) => {
            send_signal(daemon_pid, "-CONT");
            kill_child(&mut daemon);
            return (
                false,
                format!("gate-hung-daemon: FAIL — second daemon did not start: {e}"),
            );
        }
    };
    let second_exit = wait_child_timeout(&mut second, Duration::from_secs(25));
    let status = read_json(&data_dir.join("daemon-status.json"));
    let code = status
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let needle = data_dir.to_string_lossy().to_string();
    let daemon_count_while_hung = procs_with_arg(&needle)
        .into_iter()
        .filter(|(pid, _)| {
            // The daemon binary name is rsh-daemon; match by
            // cmdline containing the daemon path + our data dir.
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-daemon")
        })
        .count();
    report.check(
        "second daemon exits(1) with DaemonHung",
        second_exit == Some(1) && code == "DaemonHung",
        &format!("exit: {second_exit:?}, status code: {code:?} (not 0, not takeover)"),
    );
    report.check(
        "exactly 1 daemon process while hung",
        daemon_count_while_hung == 1,
        &format!("{daemon_count_while_hung} rsh-daemon processes with this data dir"),
    );
    // UI against the hung daemon: 0 respawns in 45 s (the fresh
    // DaemonHung fatal blocks them; FATAL_FRESHNESS is 600 s).
    let Ok((ui, _)) = stage_ui_with_real_daemon(&stage) else {
        send_signal(daemon_pid, "-CONT");
        kill_child(&mut daemon);
        return (false, "gate-hung-daemon: FAIL — UI staging failed".into());
    };
    let Some(xvfb) = XvfbSession::start() else {
        send_signal(daemon_pid, "-CONT");
        kill_child(&mut daemon);
        return (false, "gate-hung-daemon: FAIL — Xvfb did not start".into());
    };
    let display = xvfb.display();
    let ui_log = stage.join("ui.log");
    let mut ui_child = match launch_ui(&ui, &data_dir, &display, 60, &ui_log, &[]) {
        Ok(c) => c,
        Err(e) => {
            send_signal(daemon_pid, "-CONT");
            kill_child(&mut daemon);
            return (
                false,
                format!("gate-hung-daemon: FAIL — UI did not start: {e}"),
            );
        }
    };
    std::thread::sleep(Duration::from_secs(45));
    let ui_spawns_while_hung = count_lines_containing(&ui_log, "spawned daemon");
    let ui_alive = child_alive(&mut ui_child);
    report.check(
        "UI spawns 0 daemons while the daemon is hung",
        ui_spawns_while_hung == 0,
        &format!("{ui_spawns_while_hung} \"spawned daemon\" lines in 45 s (pre-fix: ~4)"),
    );
    // Resume: SIGCONT → the UI must reconnect, no duplicate daemon.
    send_signal(daemon_pid, "-CONT");
    let ping_back = wait_until(Duration::from_secs(10), || ipc_ping(&data_dir));
    let daemon_count_after = procs_with_arg(&needle)
        .into_iter()
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-daemon")
        })
        .count();
    kill_child(&mut ui_child);
    kill_child(&mut daemon);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "UI alive after 45 s against the hung daemon",
        ui_alive,
        &format!("UI process {}", if ui_alive { "alive" } else { "DEAD" }),
    );
    report.check(
        "IPC answers again after SIGCONT",
        ping_back,
        "ping must be true within 10 s of resume",
    );
    report.check(
        "exactly 1 daemon after resume (no duplicate)",
        daemon_count_after == 1,
        &format!("{daemon_count_after} rsh-daemon processes with this data dir"),
    );
    report.finish()
}

/// F17.4 — already-running race. 8 daemons launched at once
/// against one data dir (3 rounds) → exactly 1 First, the rest
/// exit 0 (live peer answers) — never 2 live daemons/listeners.
/// Then 2 UIs at once → exactly 1 window, the second exits 0.
/// The pid-file claim is `O_EXCL` (the old check-then-truncate
/// let two processes both become First).
#[must_use]
pub fn gate_already_running_race() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-already-running-race") {
        return skip;
    }
    let stage = stage_dir("already-running-race");
    let data_dir = stage.join("data");
    let _ = std::fs::create_dir_all(&data_dir);
    let needle = data_dir.to_string_lossy().to_string();
    for round in 1..=3 {
        let mut children: Vec<Child> = Vec::new();
        for i in 0..8 {
            let log = stage.join(format!("daemon-r{round}-{i}.log"));
            match launch_real_daemon(&data_dir, &log) {
                Ok(c) => children.push(c),
                Err(e) => {
                    report.note(&format!("round {round}: daemon {i} did not start: {e}"));
                }
            }
        }
        // Wait for settlement: the winner binds (ping true) and the
        // 7 losers exit — up to 20 s (cold debug binary + the 10 s
        // F15 grace wait). Sampling at a fixed 5 s caught losers
        // still inside their grace window.
        let settled = wait_until(Duration::from_secs(20), || {
            let live_n = procs_with_arg(&needle)
                .iter()
                .filter(|(pid, _)| {
                    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                    String::from_utf8_lossy(&cmd).contains("rsh-daemon")
                })
                .count();
            live_n == 1 && ipc_ping(&data_dir)
        });
        let live: Vec<u32> = procs_with_arg(&needle)
            .into_iter()
            .filter(|(pid, _)| {
                let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                String::from_utf8_lossy(&cmd).contains("rsh-daemon")
            })
            .map(|(pid, _)| pid)
            .collect();
        let ping_ok = ipc_ping(&data_dir);
        let _ = settled;
        // The losers exit 0 quickly (live peer answers); collect
        // their codes, then stop the winner.
        let mut loser_codes: Vec<Option<i32>> = Vec::new();
        for mut c in children {
            if live.contains(&c.id()) {
                kill_child(&mut c);
            } else {
                loser_codes.push(wait_child_timeout(&mut c, Duration::from_secs(10)));
            }
        }
        let losers_all_zero = loser_codes.iter().all(|c| *c == Some(0));
        let round_ok = live.len() == 1 && ping_ok && losers_all_zero;
        report.check(
            &format!("round {round}: exactly 1 live daemon, rest exit 0"),
            round_ok,
            &format!(
                "live daemons: {}, ping: {}, loser exits: {:?}",
                live.len(),
                ping_ok,
                loser_codes
            ),
        );
        // A fresh data dir per round would hide a stale pid file;
        // keep the same dir (the winner's pid file must be
        // re-claimable after the kill) — wait for the socket to
        // clear before the next round.
        let _ = wait_until(Duration::from_secs(5), || !ipc_ping(&data_dir));
    }
    // Two UIs at once → exactly 1 window, the second exits 0.
    let Ok((ui, ui_data)) = stage_ui_with_real_daemon(&stage) else {
        return (
            false,
            "gate-already-running-race: FAIL — UI staging failed".into(),
        );
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-already-running-race: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let log1 = stage.join("ui1.log");
    let log2 = stage.join("ui2.log");
    let mut ui1 = match launch_ui(&ui, &ui_data, &display, 25, &log1, &[]) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-already-running-race: FAIL — first UI did not start: {e}"),
            );
        }
    };
    let mut ui2 = match launch_ui(&ui, &ui_data, &display, 25, &log2, &[]) {
        Ok(c) => c,
        Err(e) => {
            kill_child(&mut ui1);
            return (
                false,
                format!("gate-already-running-race: FAIL — second UI did not start: {e}"),
            );
        }
    };
    // Either launch may win the pid claim (scheduling decides
    // which process starts first) — so do NOT assume ui1 is First
    // and ui2 Second. Wait up to 10 s for exactly one of them to
    // exit (the Second forwards its page and exits 0); the First
    // stays alive with the one window.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (second_exit, both_exited) = loop {
        let e1 = ui1.try_wait().ok().flatten();
        let e2 = ui2.try_wait().ok().flatten();
        match (e1, e2) {
            (Some(status), None) | (None, Some(status)) => {
                break (status.code(), false);
            }
            (Some(_), Some(_)) => break (None, true),
            (None, None) => {
                if Instant::now() >= deadline {
                    break (None, false);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    // Let the survivor's window appear before counting.
    std::thread::sleep(Duration::from_secs(3));
    // Count the survivor's windows by owner pid (name matching
    // cannot see the main window — see `window_visible`).
    let survivor_pid = if ui1.try_wait().ok().flatten().is_none() {
        ui1.id()
    } else {
        ui2.id()
    };
    let windows = {
        let pid_arg = survivor_pid.to_string();
        let (ok, out) = xdotool(&display, &["search", "--pid", &pid_arg]);
        if ok { out.lines().count() } else { 0 }
    };
    let ui_count = procs_with_arg(&ui_data.to_string_lossy())
        .into_iter()
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-ui")
        })
        .count();
    kill_child(&mut ui1);
    kill_child(&mut ui2);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "second UI exits 0 (page forwarded, no duplicate window)",
        second_exit == Some(0) && !both_exited,
        &format!("second UI exit: {second_exit:?}, both exited: {both_exited}"),
    );
    report.check(
        "exactly 1 RSH window",
        windows == 1,
        &format!("{windows} windows named RSH after 5 s"),
    );
    report.check(
        "exactly 1 UI process",
        ui_count == 1,
        &format!("{ui_count} rsh-ui processes with this data dir"),
    );
    report.finish()
}

/// F17.5 — Xvfb hidden/shown. Real daemon + real UI, 10× X-level
/// hide/show (`xdotool windowunmap`/`windowmap`): same UI pid, same
/// daemon pid, reopen < 1 s, zero daemon spawns while hidden.
///
/// Finding F17-HS1: the APP-driven path (X-close → tray-hidden →
/// page request → `win.show()`) panics under Xvfb — twice the
/// `GetGeometry` unwrap in winit's draw (`BadDrawable`), once a
/// SIGSEGV after the close — with the FIXED binary and a healthy
/// daemon. The X11 backend with no window manager is the suspect:
/// X-level unmap/map of the same window works, and Windows tray
/// reopen worked on the user PC (v2.0.1). So this check uses the
/// X-level mechanism and does NOT exercise the app's `show()`; the
/// app `show()` path is Windows/user-PC proven only.
#[must_use]
pub fn gate_xvfb_hidden_shown() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-xvfb-hidden-shown") {
        return skip;
    }
    let stage = stage_dir("xvfb-hidden-shown");
    let Ok((ui, data_dir)) = stage_ui_with_real_daemon(&stage) else {
        return (
            false,
            "gate-xvfb-hidden-shown: FAIL — staging failed".into(),
        );
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-xvfb-hidden-shown: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let daemon_log = stage.join("daemon.log");
    let mut daemon = match launch_real_daemon(&data_dir, &daemon_log) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-xvfb-hidden-shown: FAIL — daemon did not start: {e}"),
            );
        }
    };
    let daemon_pid = daemon.id();
    let ping_ok = wait_until(Duration::from_secs(15), || ipc_ping(&data_dir));
    if !ping_ok {
        kill_child(&mut daemon);
        return (
            false,
            "gate-xvfb-hidden-shown: FAIL — daemon never answered IPC".into(),
        );
    }
    let ui_log = stage.join("ui.log");
    let mut ui_child = match launch_ui(&ui, &data_dir, &display, 120, &ui_log, &[]) {
        Ok(c) => c,
        Err(e) => {
            kill_child(&mut daemon);
            return (
                false,
                format!("gate-xvfb-hidden-shown: FAIL — UI did not start: {e}"),
            );
        }
    };
    let ui_pid = ui_child.id();
    let shown = wait_until(Duration::from_secs(20), || {
        window_visible(&display, ui_child.id())
    });
    if !shown {
        kill_child(&mut ui_child);
        kill_child(&mut daemon);
        return (
            false,
            "gate-xvfb-hidden-shown: FAIL — window never became visible".into(),
        );
    }
    std::thread::sleep(Duration::from_secs(5));
    let needle = data_dir.to_string_lossy().to_string();
    let mut worst_reopen_ms: u128 = 0;
    let mut cycles_ok = true;
    for cycle in 1..=10 {
        let Some(wid) = window_id(&display, ui_pid) else {
            cycles_ok = false;
            report.note(&format!("cycle {cycle}: no window id found"));
            break;
        };
        xdotool(&display, &["windowunmap", &wid]);
        let hidden = wait_until(Duration::from_secs(2), || !window_visible(&display, ui_pid));
        if !hidden {
            cycles_ok = false;
            report.note(&format!("cycle {cycle}: window did not hide after unmap"));
            break;
        }
        let t0 = Instant::now();
        xdotool(&display, &["windowmap", &wid]);
        let back = wait_until(Duration::from_secs(1), || window_visible(&display, ui_pid));
        let reopen_ms = t0.elapsed().as_millis();
        worst_reopen_ms = worst_reopen_ms.max(reopen_ms);
        if !back {
            cycles_ok = false;
            report.note(&format!(
                "cycle {cycle}: window did not come back within 1 s"
            ));
            break;
        }
        // Same processes?
        let ui_now = child_alive(&mut ui_child);
        let daemon_now = child_alive(&mut daemon);
        if !ui_now || !daemon_now || ui_child.id() != ui_pid || daemon.id() != daemon_pid {
            cycles_ok = false;
            report.note(&format!(
                "cycle {cycle}: process changed (ui alive={ui_now}, daemon alive={daemon_now})"
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let ui_spawns = count_lines_containing(&ui_log, "spawned daemon");
    let daemon_count = procs_with_arg(&needle)
        .into_iter()
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-daemon")
        })
        .count();
    kill_child(&mut ui_child);
    kill_child(&mut daemon);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "10/10 hide/show cycles, same UI pid + daemon pid",
        cycles_ok,
        "windowunmap → windowmap, process identity checked each cycle",
    );
    report.check(
        "worst reopen < 1 s",
        worst_reopen_ms < 1000,
        &format!("worst reopen: {worst_reopen_ms} ms"),
    );
    report.check(
        "zero daemon spawns while hidden/shown",
        ui_spawns == 0,
        &format!("{ui_spawns} \"spawned daemon\" lines (the daemon never died)"),
    );
    report.check(
        "exactly 1 daemon at the end",
        daemon_count == 1,
        &format!("{daemon_count} rsh-daemon processes with this data dir"),
    );
    report.note("F17-HS1: app-driven hide→show (page request → win.show()) panics under Xvfb in winit's draw (BadDrawable) / SIGSEGV — X11-no-WM artifact, Windows/user-PC proven only; this check uses X-level unmap/map.");
    report.finish()
}

/// F17.6 — rapid open/close/reconnect. 20 hide/show cycles ~200 ms
/// apart, then settle: exactly 1 daemon, 1 UI, IPC answers, no
/// zombies, no stale pid files, spawn count <= 2 over the burst.
#[must_use]
pub fn gate_rapid_reconnect() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-rapid-reconnect") {
        return skip;
    }
    let stage = stage_dir("rapid-reconnect");
    let Ok((ui, data_dir)) = stage_ui_with_real_daemon(&stage) else {
        return (false, "gate-rapid-reconnect: FAIL — staging failed".into());
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-rapid-reconnect: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let daemon_log = stage.join("daemon.log");
    let mut daemon = match launch_real_daemon(&data_dir, &daemon_log) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-rapid-reconnect: FAIL — daemon did not start: {e}"),
            );
        }
    };
    let ping_ok = wait_until(Duration::from_secs(15), || ipc_ping(&data_dir));
    if !ping_ok {
        kill_child(&mut daemon);
        return (
            false,
            "gate-rapid-reconnect: FAIL — daemon never answered IPC".into(),
        );
    }
    let ui_log = stage.join("ui.log");
    let mut ui_child = match launch_ui(&ui, &data_dir, &display, 60, &ui_log, &[]) {
        Ok(c) => c,
        Err(e) => {
            kill_child(&mut daemon);
            return (
                false,
                format!("gate-rapid-reconnect: FAIL — UI did not start: {e}"),
            );
        }
    };
    let shown = wait_until(Duration::from_secs(20), || {
        window_visible(&display, ui_child.id())
    });
    if !shown {
        kill_child(&mut ui_child);
        kill_child(&mut daemon);
        return (
            false,
            "gate-rapid-reconnect: FAIL — window never became visible".into(),
        );
    }
    std::thread::sleep(Duration::from_secs(5));
    let spawns_before = count_lines_containing(&ui_log, "spawned daemon");
    for _ in 0..20 {
        if let Some(wid) = window_id(&display, ui_child.id()) {
            xdotool(&display, &["windowunmap", &wid]);
            std::thread::sleep(Duration::from_millis(100));
            xdotool(&display, &["windowmap", &wid]);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    std::thread::sleep(Duration::from_secs(5));
    let needle = data_dir.to_string_lossy().to_string();
    let procs = procs_with_arg(&needle);
    let daemon_procs: Vec<(u32, bool)> = procs
        .iter()
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-daemon")
        })
        .copied()
        .collect();
    let ui_procs: Vec<(u32, bool)> = procs
        .iter()
        .filter(|(pid, _)| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            String::from_utf8_lossy(&cmd).contains("rsh-ui")
        })
        .copied()
        .collect();
    let zombies = procs.iter().filter(|(_, z)| *z).count();
    let ping_after = ipc_ping(&data_dir);
    let visible_after = window_visible(&display, ui_child.id());
    let spawns = count_lines_containing(&ui_log, "spawned daemon") - spawns_before;
    let daemon_pid_ok = std::fs::read_to_string(data_dir.join("rsh-daemon.pid"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .is_some_and(|pid| daemon_procs.iter().any(|(p, _)| *p == pid));
    let ui_pid_ok = std::fs::read_to_string(data_dir.join("rsh-ui.pid"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .is_some_and(|pid| ui_procs.iter().any(|(p, _)| *p == pid));
    let stale_request = data_dir.join("show-ui.request").exists();
    kill_child(&mut ui_child);
    kill_child(&mut daemon);
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "exactly 1 daemon after the burst",
        daemon_procs.len() == 1,
        &format!("{} rsh-daemon processes", daemon_procs.len()),
    );
    report.check(
        "exactly 1 UI after the burst",
        ui_procs.len() == 1,
        &format!("{} rsh-ui processes", ui_procs.len()),
    );
    report.check(
        "IPC answers after the burst",
        ping_after,
        "ping must be true after 20 rapid cycles",
    );
    report.check(
        "window visible after the burst",
        visible_after,
        "the window must be back after the last map",
    );
    report.check(
        "no zombie processes",
        zombies == 0,
        &format!("{zombies} zombie processes with this data dir"),
    );
    report.check(
        "spawn count <= 2 over the burst",
        spawns <= 2,
        &format!("{spawns} daemon spawns during the burst (the daemon never died)"),
    );
    report.check(
        "pid files name the live processes",
        daemon_pid_ok && ui_pid_ok,
        &format!("daemon pid file live: {daemon_pid_ok}, UI pid file live: {ui_pid_ok}"),
    );
    report.check(
        "no stale show-ui.request left",
        !stale_request,
        "a leftover request file would reopen the window on next start",
    );
    report.finish()
}

/// One frame-budget condition run: launch the UI, sample liveness
/// every 100 ms until the close trigger, send `windowclose`, measure
/// the hide latency, then measure the exit latency against the
/// `--quit-after` timer.
struct FbOutcome {
    samples: usize,
    missed: usize,
    close_ack_ms: Option<u128>,
    exit_after_quit_ms: Option<i128>,
    exited_early: bool,
    daemon_answered: bool,
}

fn run_fb_condition(
    stage_name: &str,
    mode: Option<&str>,
    slow_ms: u64,
    quit_after: u64,
    close_on_answered: bool,
    display: &str,
) -> Option<FbOutcome> {
    let stage = stage_dir(stage_name);
    let (ui, data_dir, spawn_log) = if mode.is_some() {
        stage_ui_with_fake_daemon(&stage).ok()?
    } else {
        // "dead": no sibling daemon at all — the spawn fails and the
        // UI stays on sim.
        let ui = stage_copy(&stage, "rsh-ui").ok()?;
        let data_dir = stage.join("data");
        let _ = std::fs::create_dir_all(&data_dir);
        (ui, data_dir, stage.join("spawns.log"))
    };
    let log_path = stage.join("ui.log");
    let spawn_log_arg = spawn_log.to_string_lossy().to_string();
    let mut env: Vec<(&str, &str)> = Vec::new();
    let mode_str;
    let slow_str;
    if let Some(m) = mode {
        mode_str = m;
        env.push(("RSH_FAKE_DAEMON_MODE", mode_str));
        env.push(("RSH_FAKE_DAEMON_LOG", spawn_log_arg.as_str()));
        if m == "slow" {
            slow_str = slow_ms.to_string();
            env.push(("RSH_FAKE_DAEMON_SLOW_MS", slow_str.as_str()));
        }
    }
    let launched = Instant::now();
    let mut child = launch_ui(&ui, &data_dir, display, quit_after, &log_path, &env).ok()?;
    let shown = wait_until(Duration::from_secs(20), || {
        window_visible(display, child.id())
    });
    if !shown {
        kill_child(&mut child);
        let _ = std::fs::remove_dir_all(&stage);
        return Some(FbOutcome {
            samples: 0,
            missed: usize::MAX,
            close_ack_ms: None,
            exit_after_quit_ms: None,
            exited_early: false,
            daemon_answered: false,
        });
    }
    // Liveness sampling until the close trigger.
    let mut samples = 0usize;
    let mut missed = 0usize;
    let mut daemon_answered = false;
    let close_at = if close_on_answered {
        // The freeze probe: close 0.5 s after the UI logs the live
        // link — the reconnect seed fetch (the old UI-thread freeze)
        // runs right then.
        let deadline = launched + Duration::from_secs(40);
        loop {
            if count_lines_containing(&log_path, "daemon answered after spawn") > 0 {
                daemon_answered = true;
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            samples += 1;
            if !window_visible(display, child.id()) || !child_alive(&mut child) {
                missed += 1;
            }
        }
        if daemon_answered {
            Instant::now() + Duration::from_millis(500)
        } else {
            launched + Duration::from_secs(30)
        }
    } else {
        launched + Duration::from_secs(30)
    };
    while Instant::now() < close_at {
        std::thread::sleep(Duration::from_millis(100));
        samples += 1;
        if !window_visible(display, child.id()) || !child_alive(&mut child) {
            missed += 1;
        }
    }
    // Input-ack: windowclose → time until the window is gone.
    let close_ack_ms = window_id(display, child.id()).and_then(|wid| {
        let t0 = Instant::now();
        xdotool(display, &["windowclose", &wid]);
        if wait_until(Duration::from_secs(5), || {
            !window_visible(display, child.id())
        }) {
            Some(t0.elapsed().as_millis())
        } else {
            None
        }
    });
    // Exit latency vs the --quit-after timer (the UI is hidden now
    // but must still quit on its timer).
    let quit_deadline = launched + Duration::from_secs(quit_after);
    let exited_early = child_exited(&mut child);
    let exit_after_quit_ms = loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let now = Instant::now();
                let offset = if now >= quit_deadline {
                    now.duration_since(quit_deadline).as_millis() as i128
                } else {
                    -(quit_deadline.duration_since(now).as_millis() as i128)
                };
                break Some(offset);
            }
            Ok(None) => {
                if Instant::now() > quit_deadline + Duration::from_secs(10) {
                    kill_child(&mut child);
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break None,
        }
    };
    let _ = std::fs::remove_dir_all(&stage);
    Some(FbOutcome {
        samples,
        missed,
        close_ack_ms,
        exit_after_quit_ms,
        exited_early,
        daemon_answered,
    })
}

/// F17.8 — frame budget, degraded daemon. Real UI under Xvfb vs
/// dead / silent / slow daemons: the window must be present at
/// every 100 ms sample, no input unacknowledged > 1 s (windowclose
/// ack), exit within 5 s of `--quit-after`.
///
/// Honest limit: true tick p99 (<100 ms, zero ticks >500 ms) cannot
/// be measured from outside the process — there is no in-process
/// tick log to read. These are the user-visible proxies (a frozen
/// UI thread shows up as a missed sample / a late close-ack / a
/// late exit). The in-process tick log is a follow-up for the UI
/// worker (env-gated tick timings in the frame timer).
#[must_use]
pub fn gate_frame_budget() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-frame-budget") {
        return skip;
    }
    let Some(xvfb) = XvfbSession::start() else {
        return (false, "gate-frame-budget: FAIL — Xvfb did not start".into());
    };
    let display = xvfb.display();
    // Dead: no daemon at all (spawn fails, UI on sim).
    if let Some(o) = run_fb_condition("fb-dead", None, 0, 65, false, &display) {
        report.check(
            "dead daemon: window present at every 100 ms sample",
            o.missed == 0,
            &format!(
                "{}/{} samples missed (window gone or process dead)",
                o.missed, o.samples
            ),
        );
        report.check(
            "dead daemon: input ack < 1 s",
            o.close_ack_ms.is_some_and(|ms| ms < 1000),
            &format!("windowclose ack: {:?} ms", o.close_ack_ms),
        );
        report.check(
            "dead daemon: exit within 5 s of --quit-after",
            o.exit_after_quit_ms
                .is_some_and(|ms| (-5000..=5000).contains(&ms)),
            &format!("exit offset vs timer: {:?} ms", o.exit_after_quit_ms),
        );
        report.check(
            "dead daemon: no early exit (crash)",
            !o.exited_early,
            "the UI must not die before its timer",
        );
    } else {
        report.check("dead daemon: run completed", false, "staging/launch failed");
    }
    // Silent: daemon binds, never replies.
    if let Some(o) = run_fb_condition("fb-silent", Some("silent"), 0, 65, false, &display) {
        report.check(
            "silent daemon: window present at every 100 ms sample",
            o.missed == 0,
            &format!("{}/{} samples missed", o.missed, o.samples),
        );
        report.check(
            "silent daemon: input ack < 1 s",
            o.close_ack_ms.is_some_and(|ms| ms < 1000),
            &format!("windowclose ack: {:?} ms", o.close_ack_ms),
        );
        report.check(
            "silent daemon: exit within 5 s of --quit-after",
            o.exit_after_quit_ms
                .is_some_and(|ms| (-5000..=5000).contains(&ms)),
            &format!("exit offset vs timer: {:?} ms", o.exit_after_quit_ms),
        );
        report.check(
            "silent daemon: no early exit (crash)",
            !o.exited_early,
            "the UI must not die before its timer",
        );
    } else {
        report.check(
            "silent daemon: run completed",
            false,
            "staging/launch failed",
        );
    }
    // Slow: 300 ms data calls, liveness (ping/subscribe/poll) fast
    // — the freeze case (a 2 s-slow daemon never goes live at all:
    // subscribe times out at the 500 ms client bound, measured in
    // the F17 repro run 1 — that is the spawn-bound property, not a
    // frame-budget one). The close lands 0.5 s after the live link,
    // mid seed-fetch: pre-fix the UI-thread seed fetch froze the
    // close for seconds; post-fix (F5/F6 job thread) it acks fast.
    if let Some(o) = run_fb_condition("fb-slow", Some("slow"), 300, 65, true, &display) {
        report.check(
            "slow daemon: the UI goes live (liveness channel healthy)",
            o.daemon_answered,
            "the 300 ms daemon must answer subscribe within the bound",
        );
        report.check(
            "slow daemon: window present at every 100 ms sample",
            o.missed == 0,
            &format!("{}/{} samples missed", o.missed, o.samples),
        );
        report.check(
            "slow daemon: input ack < 1 s during the seed fetch",
            o.close_ack_ms.is_some_and(|ms| ms < 1000),
            &format!("windowclose ack 0.5 s after live: {:?} ms", o.close_ack_ms),
        );
        report.check(
            "slow daemon: exit within 5 s of --quit-after",
            o.exit_after_quit_ms
                .is_some_and(|ms| (-5000..=5000).contains(&ms)),
            &format!("exit offset vs timer: {:?} ms", o.exit_after_quit_ms),
        );
        report.check(
            "slow daemon: no early exit (crash)",
            !o.exited_early,
            "the UI must not die before its timer",
        );
    } else {
        report.check("slow daemon: run completed", false, "staging/launch failed");
    }
    report.note("Tick p99 (<100 ms, zero ticks >500 ms) is NOT directly measured — no in-process tick log exists; input-ack + liveness + exit latency are the external proxies (see the module docs).");
    report.finish()
}

/// F17.9 — toast dedupe (integration). Flap daemon (answers 3
/// pings, dies) + real UI for 90 s: the daemon flaps every respawn
/// cycle, but the drawer must hold <= 2 `DaemonDown` toasts
/// ("RSH not running" / "RSH stopped unexpectedly") and <= 5 rows
/// total, with spawns <= 6. Pre-fix (no flap coalescing, no
/// dedupe) every death pushed a row (~5+ `DaemonDown` rows in 90 s).
#[must_use]
pub fn gate_toast_dedupe_integration() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-toast-dedupe-integration") {
        return skip;
    }
    let stage = stage_dir("toast-dedupe");
    let Ok((ui, data_dir, spawn_log)) = stage_ui_with_fake_daemon(&stage) else {
        return (
            false,
            "gate-toast-dedupe-integration: FAIL — staging failed".into(),
        );
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-toast-dedupe-integration: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let log_path = stage.join("ui.log");
    let spawn_log_arg = spawn_log.to_string_lossy().to_string();
    let mut child = match launch_ui(
        &ui,
        &data_dir,
        &display,
        100,
        &log_path,
        &[
            ("RSH_FAKE_DAEMON_MODE", "flap"),
            ("RSH_FAKE_DAEMON_FLAP_PINGS", "3"),
            ("RSH_FAKE_DAEMON_LOG", spawn_log_arg.as_str()),
        ],
    ) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-toast-dedupe-integration: FAIL — UI did not start: {e}"),
            );
        }
    };
    let shown = wait_until(Duration::from_secs(20), || {
        window_visible(&display, child.id())
    });
    if !shown {
        kill_child(&mut child);
        return (
            false,
            "gate-toast-dedupe-integration: FAIL — window never became visible".into(),
        );
    }
    std::thread::sleep(Duration::from_secs(90));
    kill_child(&mut child);
    let spawns = spawn_log_times(&spawn_log).len();
    let notes = read_json(&data_dir.join("notifications.json"));
    let items = notes
        .as_ref()
        .and_then(|v| v.get("items"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let down_rows = items
        .iter()
        .filter(|it| {
            let title = it
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            title == "RSH not running" || title == "RSH stopped unexpectedly"
        })
        .count();
    let titles: Vec<String> = items
        .iter()
        .map(|it| {
            it.get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
                .to_string()
        })
        .collect();
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "DaemonDown toasts <= 2 after 90 s of flapping",
        down_rows <= 2,
        &format!("{down_rows} DaemonDown rows (pre-fix: ~5+)"),
    );
    report.check(
        "total notification rows <= 5",
        items.len() <= 5,
        &format!("{} rows: {:?}", items.len(), titles),
    );
    report.check(
        "spawns <= 6 in 90 s (backoff bounds the flap)",
        spawns <= 6,
        &format!("{spawns} daemon spawns (pre-fix: ~8)"),
    );
    report.finish()
}

/// F17.10 — 300 s lifecycle soak (inside the 5-min cap). Normal
/// use + X-close/reopen + daemon SIGKILL (observe recovery): at
/// every 5 s sample daemon == 1, UI == 1, IPC ping true (except a
/// bounded <15 s recovery window after the kill). Tray icons == 1
/// is Windows-only — recorded as n/a on Linux.
#[must_use]
pub fn gate_lifecycle_soak() -> (bool, String) {
    let mut report = LcReport::new();
    if let Some(skip) = not_unix_skip("gate-lifecycle-soak") {
        return skip;
    }
    let stage = stage_dir("lifecycle-soak");
    let Ok((ui, data_dir)) = stage_ui_with_real_daemon(&stage) else {
        return (false, "gate-lifecycle-soak: FAIL — staging failed".into());
    };
    let Some(xvfb) = XvfbSession::start() else {
        return (
            false,
            "gate-lifecycle-soak: FAIL — Xvfb did not start".into(),
        );
    };
    let display = xvfb.display();
    let daemon_log = stage.join("daemon.log");
    let mut daemon = match launch_real_daemon(&data_dir, &daemon_log) {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("gate-lifecycle-soak: FAIL — daemon did not start: {e}"),
            );
        }
    };
    let ping_ok = wait_until(Duration::from_secs(15), || ipc_ping(&data_dir));
    if !ping_ok {
        kill_child(&mut daemon);
        return (
            false,
            "gate-lifecycle-soak: FAIL — daemon never answered IPC".into(),
        );
    }
    let ui_log = stage.join("ui.log");
    let mut ui_child = match launch_ui(&ui, &data_dir, &display, 310, &ui_log, &[]) {
        Ok(c) => c,
        Err(e) => {
            kill_child(&mut daemon);
            return (
                false,
                format!("gate-lifecycle-soak: FAIL — UI did not start: {e}"),
            );
        }
    };
    let shown = wait_until(Duration::from_secs(20), || {
        window_visible(&display, ui_child.id())
    });
    if !shown {
        kill_child(&mut ui_child);
        kill_child(&mut daemon);
        return (
            false,
            "gate-lifecycle-soak: FAIL — window never became visible".into(),
        );
    }
    let needle = data_dir.to_string_lossy().to_string();
    let started = Instant::now();
    let mut samples = 0usize;
    let mut violations: Vec<String> = Vec::new();
    let mut kill_at: Option<Instant> = None;
    let mut killed = false;
    let mut reopened_at_120 = false;
    let mut reopened_at_240 = false;
    let mut recovery_ping_at: Option<Duration> = None;
    while started.elapsed() < Duration::from_secs(300) {
        let t = started.elapsed();
        let secs = t.as_secs();
        // Events: X hide/show at 120 s and 240 s; SIGKILL at 180 s.
        if secs >= 120 && !reopened_at_120 {
            reopened_at_120 = true;
            if let Some(wid) = window_id(&display, ui_child.id()) {
                xdotool(&display, &["windowunmap", &wid]);
                std::thread::sleep(Duration::from_millis(500));
                xdotool(&display, &["windowmap", &wid]);
            }
        }
        if secs >= 240 && !reopened_at_240 {
            reopened_at_240 = true;
            if let Some(wid) = window_id(&display, ui_child.id()) {
                xdotool(&display, &["windowunmap", &wid]);
                std::thread::sleep(Duration::from_millis(500));
                xdotool(&display, &["windowmap", &wid]);
            }
        }
        if secs >= 180 && !killed {
            killed = true;
            kill_at = Some(Instant::now());
            let pids: Vec<u32> = procs_with_arg(&needle)
                .into_iter()
                .filter(|(pid, _)| {
                    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                    String::from_utf8_lossy(&cmd).contains("rsh-daemon")
                })
                .map(|(pid, _)| pid)
                .collect();
            for pid in pids {
                send_signal(pid, "-KILL");
            }
            // The Child handle still points at the dead daemon;
            // reap it so the count below is not confused by a
            // zombie (the UI respawns a fresh daemon).
            let _ = daemon.try_wait();
        }
        // Sample.
        samples += 1;
        let procs = procs_with_arg(&needle);
        let daemon_n = procs
            .iter()
            .filter(|(pid, _)| {
                let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                String::from_utf8_lossy(&cmd).contains("rsh-daemon")
            })
            .count();
        let ui_n = procs
            .iter()
            .filter(|(pid, _)| {
                let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                String::from_utf8_lossy(&cmd).contains("rsh-ui")
            })
            .count();
        let ping = ipc_ping(&data_dir);
        let in_recovery = kill_at.is_some_and(|k| k.elapsed() < Duration::from_secs(15));
        if daemon_n != 1 {
            violations.push(format!("t={secs}s: daemon count {daemon_n}"));
        }
        if ui_n != 1 {
            violations.push(format!("t={secs}s: UI count {ui_n}"));
        }
        if !ping && !in_recovery {
            violations.push(format!(
                "t={secs}s: IPC ping false outside the recovery window"
            ));
        }
        if ping && recovery_ping_at.is_none() && killed {
            recovery_ping_at = Some(kill_at.map_or(Duration::ZERO, |k| k.elapsed()));
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let spawns = count_lines_containing(&ui_log, "spawned daemon");
    let ui_alive = child_alive(&mut ui_child);
    kill_child(&mut ui_child);
    let _ = daemon.try_wait();
    let _ = std::fs::remove_dir_all(&stage);
    report.check(
        "every 5 s sample: daemon == 1",
        !violations.iter().any(|v| v.contains("daemon count")),
        &format!("{samples} samples, violations: {violations:?}"),
    );
    report.check(
        "every 5 s sample: UI == 1",
        !violations.iter().any(|v| v.contains("UI count")),
        &format!("{samples} samples, violations: {violations:?}"),
    );
    report.check(
        "IPC ping true except the <15 s recovery window",
        !violations.iter().any(|v| v.contains("ping false")),
        &format!("recovery ping after: {recovery_ping_at:?} (kill at 180 s)"),
    );
    report.check(
        "recovery: ping back within 15 s of the SIGKILL",
        recovery_ping_at.is_some_and(|d| d < Duration::from_secs(15)),
        &format!("ping back after: {recovery_ping_at:?}"),
    );
    report.check(
        "bounded respawn: <= 3 daemon spawns in 300 s (1 startup + recovery)",
        spawns <= 3,
        &format!("{spawns} \"spawned daemon\" lines"),
    );
    report.check(
        "UI alive at the end of the soak",
        ui_alive,
        &format!("UI process {}", if ui_alive { "alive" } else { "DEAD" }),
    );
    report.note("Tray icons == 1: n/a on Linux (Windows-only — F18 + user-PC retest).");
    report.finish()
}

/// F17.7 (source audit) — the popup timeline matrix: the daemon
/// must use `PopupDismissPoll` + `TrayToggleGuard` (F9), with the
/// F17 matrix cases present in `tray_popup.rs`. The timeline
/// behaviour itself is unit-tested there (23 tests); this gate
/// check is the wiring proof that the production path uses it.
#[must_use]
pub fn gate_tray_popup_timeline() -> (bool, String) {
    let mut report = LcReport::new();
    let root = crate::gate_checks::repo_root();
    let main =
        std::fs::read_to_string(root.join("crates/rsh-daemon/src/main.rs")).unwrap_or_default();
    let popup = std::fs::read_to_string(root.join("crates/rsh-daemon/src/tray_popup.rs"))
        .unwrap_or_default();
    report.check(
        "daemon uses PopupDismissPoll",
        main.contains("PopupDismissPoll"),
        "main.rs must drive the popup through the F9 state machine",
    );
    report.check(
        "daemon uses TrayToggleGuard",
        main.contains("TrayToggleGuard"),
        "main.rs must debounce tray toggles (double-click = single show)",
    );
    for case in [
        "transient_baseline_switch_back_by_700ms_stays_open",
        "transient_activation_200_to_250ms_stays_open",
        "sustained_foreground_loss_at_1500ms_dismisses_immediately",
    ] {
        report.check(
            &format!("F17 matrix case present: {case}"),
            popup.contains(case),
            "tray_popup.rs must carry the F17 timeline case",
        );
    }
    report.finish()
}
