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
                    loop {
                        let mut server = match ServerOptions::new()
                            .first_pipe_instance(true)
                            .create(SINGLE_PIPE_NAME)
                        {
                            Ok(server) => server,
                            Err(error) => {
                                eprintln!("RSH-UI single-instance pipe unavailable: {error}");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        };
                        if server.connect().await.is_err() {
                            continue;
                        }
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
    if let Ok(text) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if pid != std::process::id() && pid_is_live_rsh_ui(pid) {
                // Second instance: forward the page and exit — never
                // a duplicate window.
                let _ = std::fs::write(dir.join("show-ui.request"), format!("{page}\n"));
                return InstanceRole::Second;
            }
        }
    }
    // First instance (or a stale pid file): claim it.
    let _ = std::fs::write(&pid_path, std::process::id().to_string());
    InstanceRole::First(InstanceGuard::with_dir(Some(dir.clone())))
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
        // A live process whose cmdline names rsh-ui (daemon test
        // pattern: `exec -a` renames argv[0] without running the
        // real UI).
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exec -a rsh-ui sleep 30"])
            .spawn()
            .expect("spawn fake rsh-ui");
        let pid = child.id();
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
