//! UI single-instance + page-forward (M-5/A1, v2.0.2 — Tray-team hook).
//!
//! The daemon spawns `rsh-ui --page <page>` for every tray action
//! (Open RSH, Settings, Run setup again) even when the UI is already
//! running. A second instance must never open a duplicate window:
//! it forwards the page to the first instance and exits 0.
//!
//! * Windows: the first instance holds the named mutex
//!   `Global\RSH-UI-<USERNAME>` (daemon `single_instance.rs`
//!   pattern); a second instance writes JSON `{"page": "..."}` to
//!   the named pipe `\\.\pipe\rsh-ui-single`; the first instance
//!   listens on a background thread and queues requests for the UI
//!   thread (drained by the frame timer).
//! * Linux: pid file `<data_dir>/rsh-ui.pid` (liveness via
//!   `/proc/<pid>/cmdline`, same pattern as the daemon); a second
//!   instance writes the page name to `<data_dir>/show-ui.request`;
//!   the first instance polls + consumes it in the frame timer.
//!
//! Quit: the daemon writes `<data_dir>/rsh-ui.quit` on tray "Quit
//! RSH"; the UI consumes it in the frame timer and quits its event
//! loop. A stale quit file is cleared at startup, so a crash never
//! makes the next launch quit immediately.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::Duration;

/// The UI window title (Slint `title:`) — the Windows
/// foreground/restore helper finds the window by this title.
#[cfg(windows)]
pub const UI_WINDOW_TITLE: &str = "RSH — Real Sim Hub";

/// Page names the tray/daemon can request (the `--page` values).
#[cfg(windows)]
const KNOWN_PAGES: &[&str] = &[
    "setup", "overview", "steering", "pedals", "buttons", "extras", "settings", "about",
];

/// Page name → shell page index (mirrors `parse_args` in `main.rs`).
#[must_use]
pub fn page_index(page: &str) -> i32 {
    match page {
        "steering" => 1,
        "pedals" => 2,
        "buttons" => 3,
        "extras" => 4,
        "settings" => 5,
        "about" => 6,
        _ => 0,
    }
}

/// Shell page index + setup flag → page name (for forwarding the
/// startup request to a running instance).
#[must_use]
pub fn page_name(page: i32, setup: bool) -> &'static str {
    if setup {
        return "setup";
    }
    match page {
        1 => "steering",
        2 => "pedals",
        3 => "buttons",
        4 => "extras",
        5 => "settings",
        6 => "about",
        _ => "overview",
    }
}

/// Whether the parsed page payload names a known page (Windows
/// pipe path only).
#[cfg(windows)]
fn parse_page_payload(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(page) = value.get("page").and_then(serde_json::Value::as_str) {
            if KNOWN_PAGES.contains(&page) {
                return Some(page.to_string());
            }
        }
    }
    if KNOWN_PAGES.contains(&trimmed) {
        return Some(trimmed.to_string());
    }
    None
}

/// The outcome of the single-instance check, decided BEFORE the
/// window is created.
pub enum InstanceRole {
    /// This process is the first UI instance; hold the guard for
    /// the process lifetime (dropping it releases the mutex / pid
    /// file).
    First(InstanceGuard),
    /// Another UI instance is running and got the page request;
    /// this process must exit 0 without creating a window.
    Second,
}

/// Holds the single-instance claim: the Windows mutex handle, or
/// the Linux pid file (removed on drop).
pub struct InstanceGuard {
    #[cfg(windows)]
    handle: Option<windows::Win32::Foundation::HANDLE>,
    #[cfg(not(windows))]
    data_dir: Option<PathBuf>,
}

impl InstanceGuard {
    #[cfg(windows)]
    fn with_handle(handle: windows::Win32::Foundation::HANDLE) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    #[cfg(windows)]
    fn empty() -> Self {
        Self { handle: None }
    }

    #[cfg(not(windows))]
    fn with_dir(data_dir: Option<PathBuf>) -> Self {
        Self { data_dir }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
        #[cfg(not(windows))]
        if let Some(dir) = &self.data_dir {
            let _ = std::fs::remove_file(dir.join("rsh-ui.pid"));
        }
    }
}

/// Become the single UI instance, or forward `page` to the running
/// one. Call BEFORE creating the window; on [`InstanceRole::Second`]
/// the caller exits 0.
#[must_use]
pub fn ensure_single_instance(data_dir: Option<&PathBuf>, page: &str) -> InstanceRole {
    // Windows ignores the data dir (named mutex + pipe instead of
    // the pid file); keep the parameter for the shared signature.
    #[cfg(windows)]
    let _ = data_dir;
    #[cfg(windows)]
    {
        ensure_single_instance_windows(page)
    }
    #[cfg(not(windows))]
    {
        ensure_single_instance_unix(data_dir, page)
    }
}

#[cfg(windows)]
fn ensure_single_instance_windows(page: &str) -> InstanceRole {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    // Per-user mutex name, mirroring the daemon's
    // `Global\RSH-Daemon-<user>` (shared machines stay independent).
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
    let name: Vec<u16> = format!("Global\\RSH-UI-{user}\0").encode_utf16().collect();
    let handle = match unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) } {
        Ok(handle) => handle,
        Err(error) => {
            // Cannot determine single-instance state: open the
            // window anyway (a duplicate is better than no UI).
            eprintln!("RSH-UI single-instance mutex unavailable: {error}");
            return InstanceRole::First(InstanceGuard::empty());
        }
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        forward_page_windows(page);
        return InstanceRole::Second;
    }
    InstanceRole::First(InstanceGuard::with_handle(handle))
}

/// Windows named-pipe name for page forwarding (the spec's exact
/// name; pipes are per-session, so no user suffix is needed).
#[cfg(windows)]
const SINGLE_PIPE_NAME: &str = r"\\.\pipe\rsh-ui-single";

/// Second instance (Windows): deliver `page` to the first instance
/// over the named pipe (3 attempts, ~2 s each — the first instance
/// may still be starting its listener). Never opens a window.
#[cfg(windows)]
fn forward_page_windows(page: &str) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ClientOptions;

    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let forwarded = rt.block_on(async {
        for _ in 0..3 {
            match tokio::time::timeout(
                Duration::from_secs(2),
                std::future::ready(ClientOptions::new().open(SINGLE_PIPE_NAME)),
            )
            .await
            {
                Ok(Ok(mut client)) => {
                    let payload = serde_json::json!({"page": page}).to_string();
                    return client.write_all(payload.as_bytes()).await.is_ok()
                        && client.flush().await.is_ok();
                }
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        false
    });
    if !forwarded {
        eprintln!("RSH-UI single-instance: could not forward page '{page}' to the running UI");
    }
}

/// First instance (Windows): listen on the named pipe in the
/// background and queue page requests for the UI thread. Linux is
/// a no-op (the frame timer polls the request file instead).
/// The `Arc` is moved into the listener thread on Windows — the
/// lint only sees the Linux no-op body.
#[allow(clippy::needless_pass_by_value)]
pub fn listen_for_page_requests(queue: Arc<Mutex<VecDeque<String>>>) {
    #[cfg(windows)]
    {
        let _ = std::thread::Builder::new()
            .name("rsh-ui-single".to_string())
            .spawn(move || {
                use tokio::io::AsyncReadExt;
                use tokio::net::windows::named_pipe::ServerOptions;

                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    // DS-33 (F20-C1): backoff ramp 1 s -> 30 s cap +
                    // quiet-after-first-log — NOT a latch. The old
                    // code logged every create failure and retried at
                    // a fixed 1 s, and a failing `connect()` spun
                    // with no sleep at all. The loop persists while
                    // the UI lives; a permanently broken pipe settles
                    // at one attempt per 30 s. A success resets both
                    // the ramp and the log flag, so a later incident
                    // logs once again.
                    let backoff = |failures: u32| -> Duration {
                        Duration::from_secs(
                            1u64.saturating_mul(
                                2u64.saturating_pow(failures.saturating_sub(1).min(5)),
                            )
                            .min(30),
                        )
                    };
                    let mut failures = 0u32;
                    let mut logged = false;
                    loop {
                        let mut server = match ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(SINGLE_PIPE_NAME)
                        {
                            Ok(server) => server,
                            Err(error) => {
                                if !logged {
                                    eprintln!("RSH-UI single-instance pipe unavailable: {error}");
                                    logged = true;
                                }
                                failures = failures.saturating_add(1);
                                tokio::time::sleep(backoff(failures)).await;
                                continue;
                            }
                        };
                        if server.connect().await.is_err() {
                            // Usually a racing second instance; back
                            // off the same way instead of spinning
                            // (the old code had no sleep here).
                            failures = failures.saturating_add(1);
                            tokio::time::sleep(backoff(failures)).await;
                            continue;
                        }
                        failures = 0;
                        logged = false;
                        let mut buf = Vec::new();
                        let read = tokio::time::timeout(
                            Duration::from_secs(5),
                            server.read_to_end(&mut buf),
                        )
                        .await;
                        if read.is_ok_and(|result| result.is_ok()) {
                            if let Some(page) = parse_page_payload(&buf) {
                                if let Ok(mut pending) = queue.lock() {
                                    pending.push_back(page);
                                }
                            }
                        }
                        let _ = server.disconnect();
                    }
                });
            });
    }
    #[cfg(not(windows))]
    {
        let _ = queue;
    }
}

/// First instance (Windows): show + restore + foreground the UI
/// window by title (mirrors the daemon's `restore_ui_window`).
/// `win.show()` already makes the window visible; this forces it to
/// the front, which a tray-open window needs.
#[cfg(windows)]
pub fn restore_and_foreground() {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(UI_WINDOW_TITLE)) }.unwrap_or_default();
    if hwnd.0.is_null() {
        return;
    }
    let command = if unsafe { IsWindowVisible(hwnd) }.as_bool() {
        SW_RESTORE
    } else {
        SW_SHOW
    };
    let _ = unsafe { ShowWindow(hwnd, command) };
    let _ = unsafe { SetForegroundWindow(hwnd) };
}

/// R3-UI-05 (Windows): the UI window's native handle, found by
/// title (same lookup as `restore_and_foreground`) — the frame
/// timer compares the foreground window against it. `None` when
/// the window does not exist yet (Slint creates it after
/// `show()`); the caller retries and fails open.
#[cfg(windows)]
#[must_use]
pub fn ui_window_hwnd() -> Option<isize> {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    let hwnd = unsafe { FindWindowW(None, &HSTRING::from(UI_WINDOW_TITLE)) }.unwrap_or_default();
    if hwnd.0.is_null() {
        None
    } else {
        Some(hwnd.0 as isize)
    }
}

/// R3-UI-05 (Windows): whether the given native window is the
/// current foreground window. A minimized window is never
/// foreground, so this one check covers both unfocused and
/// minimized.
#[cfg(windows)]
#[must_use]
pub fn window_is_foreground(hwnd: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let foreground = unsafe { GetForegroundWindow() };
    !foreground.is_invalid() && foreground.0 as isize == hwnd
}

/// Whether `pid` is a live `rsh-ui` process (Linux pid-file check,
/// mirroring the daemon's `pid_is_live_rsh_daemon`): `/proc/<pid>`
/// merely existing is not enough (recycled pids, zombies), and an
/// unreadable command line is treated as live (conservative — never
/// steal a lock we cannot identify).
#[cfg(unix)]
fn pid_is_live_rsh_ui(pid: u32) -> bool {
    use std::path::Path;
    if !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    // Zombie: dead, but /proc still lists it.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        let state = stat
            .rsplit(") ")
            .next()
            .and_then(|rest| rest.chars().next());
        if state == Some('Z') {
            return false;
        }
    }
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(cmdline) => String::from_utf8_lossy(&cmdline)
            .split('\0')
            .any(|arg| arg.contains("rsh-ui")),
        Err(_) => true,
    }
}

#[cfg(not(windows))]
fn ensure_single_instance_unix(data_dir: Option<&PathBuf>, page: &str) -> InstanceRole {
    let Some(dir) = data_dir else {
        // No data dir: no coordination possible — open the window.
        return InstanceRole::First(InstanceGuard::with_dir(None));
    };
    let pid_path = dir.join("rsh-ui.pid");
    // F17 (v2.0.3): the pid claim is `O_EXCL` create — the old
    // check-then-`fs::write` (truncate) let two UIs launched at
    // once both become First (duplicate windows). Same shape as
    // the daemon's `single_instance::claim_pid_file`: exactly one
    // caller wins the claim; a loser re-reads (a live winner
    // forwards the page, a stale file is removed and the claim
    // retried, bounded).
    for _ in 0..16 {
        if pid_file_names_live_ui(&pid_path) {
            // Second instance: forward the page and exit — never
            // a duplicate window.
            let _ = std::fs::write(dir.join("show-ui.request"), format!("{page}\n"));
            return InstanceRole::Second;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pid_path)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                let _ = writeln!(file, "{}", std::process::id());
                return InstanceRole::First(InstanceGuard::with_dir(Some(dir.clone())));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Lost the claim race. The winner may still be
                // writing its pid (create_new done, writeln
                // pending) — the old code waited once (10 ms) and
                // then REMOVED the file when the re-check saw no
                // live pid, so a winner stalled >10 ms between
                // create and write lost its claim and the loser
                // became First too (tun-racefix: the rig's
                // `gate-already-running-race` hit this 3/3 — both
                // UIs became First and both quit on the
                // `--quit-after` timer at ~25.3 s). Wait for the
                // write instead, bounded (`PID_WRITE_GRACE_POLLS`):
                // only remove when the file names a dead pid,
                // vanishes, or is still empty after the full grace
                // (an owner that died mid-write). Never remove a
                // file that might belong to a live winner.
                let mut waited: u32 = 0;
                loop {
                    match pid_file_state(&pid_path) {
                        PidFileState::Live => {
                            let _ =
                                std::fs::write(dir.join("show-ui.request"), format!("{page}\n"));
                            return InstanceRole::Second;
                        }
                        PidFileState::Dead | PidFileState::Missing => {
                            let _ = std::fs::remove_file(&pid_path);
                            break;
                        }
                        PidFileState::Empty => {
                            waited += 1;
                            if waited >= PID_WRITE_GRACE_POLLS {
                                // Owner died mid-write (or the file is
                                // garbage): reclaim it and retry the claim.
                                let _ = std::fs::remove_file(&pid_path);
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }
            }
            Err(_) => {
                // Cannot determine single-instance state: open the
                // window anyway (a duplicate is better than no UI).
                return InstanceRole::First(InstanceGuard::with_dir(Some(dir.clone())));
            }
        }
    }
    // The claim race never settled: open the window anyway (a
    // duplicate is better than no UI).
    InstanceRole::First(InstanceGuard::with_dir(Some(dir.clone())))
}

/// How long a claim loser waits for the winner's pid to appear in
/// the pid file before concluding the file is stale (tun-racefix):
/// 200 polls x 10 ms = 2 s. The winner writes its pid within
/// microseconds of creating the file, so 2 s is already
/// pathological; it only covers a winner stalled between
/// `create_new` and `writeln!` by scheduler / filesystem stalls
/// under load.
#[cfg(not(windows))]
const PID_WRITE_GRACE_POLLS: u32 = 200;

/// What the pid file says right now (Linux single-instance claim).
/// `Empty` covers a file the winner created but has not written
/// yet (`create_new` done, `writeln!` pending) as well as garbage
/// content — both mean "no pid to check", never "no owner".
#[cfg(not(windows))]
enum PidFileState {
    /// Names a live `rsh-ui` process other than this one.
    Live,
    /// Names a dead pid (stale — safe to remove).
    Dead,
    /// Exists but holds no parsable pid (winner mid-write).
    Empty,
    /// No file (removed by a rival, or never created).
    Missing,
}

/// Classify the pid file's current content.
#[cfg(not(windows))]
fn pid_file_state(pid_path: &std::path::Path) -> PidFileState {
    let text = match std::fs::read_to_string(pid_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PidFileState::Missing;
        }
        Err(_) => return PidFileState::Empty,
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return PidFileState::Empty;
    };
    if pid == std::process::id() {
        // Our own pid (a stale file naming a recycled pid): we
        // have not claimed yet, so the file cannot be our claim
        // — it is stale and safe to remove at once.
        return PidFileState::Dead;
    }
    if pid_is_live_rsh_ui(pid) {
        PidFileState::Live
    } else {
        PidFileState::Dead
    }
}

/// Whether the pid file names a live `rsh-ui` process other than
/// this one.
#[cfg(not(windows))]
fn pid_file_names_live_ui(pid_path: &std::path::Path) -> bool {
    matches!(pid_file_state(pid_path), PidFileState::Live)
}

/// Consume a pending page request (Linux request file; Windows uses
/// the pipe, so this is always `None` there). The file is removed
/// on read, so each request is delivered once.
#[must_use]
pub fn poll_page_request_file(data_dir: Option<&PathBuf>) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = data_dir;
        None
    }
    #[cfg(not(windows))]
    {
        let dir = data_dir?;
        let path = dir.join("show-ui.request");
        let text = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        let page = text.trim().to_string();
        if page.is_empty() {
            None
        } else {
            Some(page)
        }
    }
}

/// Consume the daemon's tray-Quit request (`<data_dir>/rsh-ui.quit`,
/// written by the daemon on "Quit RSH"): `true` once per file.
/// Callers quit the event loop on `true`. Call once at startup too,
/// so a stale file from a crashed session never quits the next
/// launch immediately.
#[must_use]
pub fn take_quit_request(data_dir: Option<&PathBuf>) -> bool {
    let Some(dir) = data_dir else {
        return false;
    };
    let path = dir.join("rsh-ui.quit");
    if path.exists() {
        let _ = std::fs::remove_file(path);
        true
    } else {
        false
    }
}

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rsh-ui-lifecycle-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn page_index_maps_known_pages() {
        assert_eq!(page_index("overview"), 0);
        assert_eq!(page_index("steering"), 1);
        assert_eq!(page_index("settings"), 5);
        assert_eq!(page_index("about"), 6);
        assert_eq!(page_index("bogus"), 0);
        assert_eq!(page_name(5, false), "settings");
        assert_eq!(page_name(0, true), "setup");
        assert_eq!(page_name(2, false), "pedals");
    }

    #[test]
    fn second_instance_forwards_and_first_claims() {
        let dir = tmp_dir();
        // A live process whose cmdline names rsh-ui (argv[0] set
        // directly — no shell: `sh` here is dash, which has no
        // `exec -a`, so the old `sh -c "exec -a ..."` form died
        // instantly and the pid was reclaimed as First).
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .arg0("rsh-ui")
            .spawn()
            .expect("spawn fake rsh-ui");
        let pid = child.id();
        // Give the exec a moment to happen before we read cmdline:
        // right after spawn the child's cmdline is still the
        // parent's (no "rsh-ui" in it), so the liveness check would
        // misread it as a stale pid. The old code's single 10 ms
        // re-check accidentally masked this; the daemon's
        // equivalent test waits the same 200 ms.
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::write(dir.join("rsh-ui.pid"), pid.to_string()).unwrap();
        let role = ensure_single_instance(Some(&dir), "settings");
        assert!(matches!(role, InstanceRole::Second));
        assert_eq!(
            std::fs::read_to_string(dir.join("show-ui.request"))
                .unwrap()
                .trim(),
            "settings"
        );
        // The request file is consumed once.
        assert_eq!(
            poll_page_request_file(Some(&dir)).as_deref(),
            Some("settings")
        );
        assert_eq!(poll_page_request_file(Some(&dir)), None);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_pid_file_with_late_writer_is_not_stolen() {
        // tun-racefix: the winner's pid file is empty between
        // `create_new` and `writeln!`; a loser that re-checks in
        // that window must WAIT for the write, not remove the file
        // and become a second First (the rig's UI race hit this
        // 3/3). The writer lands 200 ms after the empty file
        // appears — past the old code's single 10 ms re-check, so
        // this test FAILS on the old code (First) and passes on the
        // fix (Second).
        let dir = tmp_dir();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .arg0("rsh-ui")
            .spawn()
            .expect("spawn fake rsh-ui");
        let pid = child.id();
        std::fs::write(dir.join("rsh-ui.pid"), "").unwrap();
        let dir2 = dir.clone();
        std::thread::spawn(move || {
            // 200 ms: the fake process has exec'd by then (its
            // cmdline names rsh-ui), and 200 ms is past the old
            // 10 ms re-check but far inside the 2 s grace.
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::write(dir2.join("rsh-ui.pid"), pid.to_string()).unwrap();
        });
        let role = ensure_single_instance(Some(&dir), "settings");
        assert!(matches!(role, InstanceRole::Second));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_pid_file_with_no_writer_is_reclaimed() {
        // An owner that died between create and write leaves an empty
        // file: the claim must still settle (bounded wait), not hang.
        // Takes the ~2 s grace.
        let dir = tmp_dir();
        std::fs::write(dir.join("rsh-ui.pid"), "").unwrap();
        let role = ensure_single_instance(Some(&dir), "overview");
        assert!(matches!(role, InstanceRole::First(_)));
        assert_eq!(
            std::fs::read_to_string(dir.join("rsh-ui.pid"))
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap(),
            std::process::id()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_pid_file_is_reclaimed() {
        let dir = tmp_dir();
        // A pid that cannot exist (far above any real pid).
        std::fs::write(dir.join("rsh-ui.pid"), "4294967295").unwrap();
        let role = ensure_single_instance(Some(&dir), "overview");
        assert!(matches!(role, InstanceRole::First(_)));
        // The stale file is replaced by this process's pid.
        assert_eq!(
            std::fs::read_to_string(dir.join("rsh-ui.pid"))
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap(),
            std::process::id()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quit_request_is_consumed_once() {
        let dir = tmp_dir();
        assert!(!take_quit_request(Some(&dir)));
        std::fs::write(dir.join("rsh-ui.quit"), "").unwrap();
        assert!(take_quit_request(Some(&dir)));
        assert!(!take_quit_request(Some(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
