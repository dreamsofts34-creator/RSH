//! UDP port-conflict diagnosis (v2.0.3 F21).
//!
//! The wheel stream port ([`crate::udp::STREAM_PORT`], 4211) is bound
//! with a plain `UdpSocket::bind` — deliberately **without**
//! `SO_REUSEADDR`. On Windows `SO_REUSEADDR` permits a second socket
//! to bind the same UDP port, so setting it would let two RSH
//! processes race for the board's stream (each receiving part of it)
//! instead of failing loudly; on Linux it would not help either (a
//! live holder still blocks the bind). A bind failure is therefore a
//! hard fact — something else already owns the port — and the fix is
//! to **name the holder**, not to retry or share the port.
//!
//! Evidence (user log `rsh-20261116_4_cj51.log`, grep-verified):
//! `AddrInUse` 117× (36 in the v2.0.1 block, 81 in the v2.0.2 block),
//! spanning ~35 min while the daemon restarted 36× — the holder
//! survived every daemon restart, so it is a process *outside* the
//! restart cycle. The only RSH process alive before the first
//! `AddrInUse` (11:39:39, 5 s after the first v2.0.1 daemon start)
//! is the legacy Tauri app (`RSH v1.0.2 starting` 09:40, no exit line
//! in the whole file): it binds the same port at launch
//! (`src-tauri/src/app_state.rs`: engine start + best-effort UDP
//! connect), so a still-running v1.0.2 app holds UDP 4211 and the
//! new daemon can never bind it.
//!
//! [`diagnose_udp_port_holder`] identifies the holder best-effort:
//! Linux reads `/proc/net/udp` + `/proc/<pid>/fd` (pure std, fully
//! testable here); Windows uses `GetExtendedUdpTable` with
//! `UDP_TABLE_OWNER_PID` plus `QueryFullProcessImageNameW` (same
//! `windows`-crate pattern as the `HIDMaestro` loopback-peer check).
//! Anything it cannot prove returns `None` — never a guess.

use std::fmt;

/// A process holding a UDP port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortHolder {
    /// OS process id of the holder.
    pub pid: u32,
    /// Process name as reported by the OS (`comm` / image file name).
    pub process_name: String,
    /// Full executable path when the OS disclosed it.
    pub exe_path: Option<String>,
}

/// Installed legacy Tauri app exe (v1.0.2): Tauri names the bundled
/// exe after `productName` (`RSH`, see the v1.0.2 release workflow:
/// the sidecar "lands beside RSH.exe").
pub const LEGACY_APP_EXE: &str = "RSH.exe";
/// Dev binary name of the same app (`src-tauri` package `rsh-app`):
/// matched too, so a dev-built old app is recognised as well.
pub const LEGACY_APP_DEV_EXE: &str = "rsh-app.exe";
/// Current-stack binaries (v2.0 daemon + UI).
pub const CURRENT_DAEMON_EXE: &str = "rsh-daemon";
/// Current-stack binaries (v2.0 daemon + UI).
pub const CURRENT_UI_EXE: &str = "rsh-ui";

impl PortHolder {
    /// `true` when the holder looks like the legacy Tauri RSH app
    /// (v1.0.2) — the prime suspect from the user log. Matches the
    /// installed exe (`RSH.exe`) and the dev binary (`rsh-app.exe`),
    /// case-insensitively, by file name (full path or bare name).
    #[must_use]
    pub fn is_legacy_rsh_app(&self) -> bool {
        let name = file_name_of(&self.process_name);
        let path_name = self
            .exe_path
            .as_deref()
            .map(file_name_of)
            .unwrap_or_default();
        [name, path_name].iter().any(|candidate| {
            candidate.eq_ignore_ascii_case(LEGACY_APP_EXE)
                || candidate.eq_ignore_ascii_case(LEGACY_APP_DEV_EXE)
        })
    }

    /// `true` when the holder is our own current stack (a second
    /// `rsh-daemon` / `rsh-ui`) — i.e. two v2.0 processes racing,
    /// not the legacy app.
    #[must_use]
    pub fn is_current_rsh(&self) -> bool {
        let base = |name: &str| {
            let lower = file_name_of(name).to_ascii_lowercase();
            let lower = lower.strip_suffix(".exe").unwrap_or(&lower);
            lower == CURRENT_DAEMON_EXE || lower == CURRENT_UI_EXE
        };
        base(&self.process_name) || self.exe_path.as_deref().is_some_and(base)
    }

    /// One-line human description for logs and the engine status
    /// detail: `RSH.exe (pid 1234)` plus the path when known.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.exe_path {
            Some(path) => format!("{} (pid {}) at {path}", self.process_name, self.pid),
            None => format!("{} (pid {})", self.process_name, self.pid),
        }
    }
}

/// File name of a path or bare name (`C:\…\RSH.exe` → `RSH.exe`).
fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Best-effort: find the process holding UDP `port` (any local IP).
///
/// Returns `None` when the port is free, when the holder cannot be
/// identified (permissions, raced exit, unsupported platform), or
/// when the platform lookup fails — a missing answer is never
/// presented as a holder. Linux is implemented with `/proc` (pure
/// std); Windows with `GetExtendedUdpTable` (`UDP_TABLE_OWNER_PID`).
#[must_use]
pub fn diagnose_udp_port_holder(port: u16) -> Option<PortHolder> {
    #[cfg(target_os = "linux")]
    {
        diagnose_linux(port)
    }
    #[cfg(windows)]
    {
        diagnose_windows(port)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = port;
        None
    }
}

#[cfg(target_os = "linux")]
fn diagnose_linux(port: u16) -> Option<PortHolder> {
    let inode = udp_listener_inode(port)?;
    let pid = pid_holding_socket(inode)?;
    let process_name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty())?;
    let exe_path = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    Some(PortHolder {
        pid,
        process_name,
        exe_path,
    })
}

#[cfg(target_os = "linux")]
fn udp_listener_inode(port: u16) -> Option<u64> {
    // /proc/net/udp lines: `sl local_address rem_address st … inode …`
    // local_address is `HEXIP:HEXPORT` (IP little-endian, port big-endian
    // hex); field 10 (0-based 9) is the socket inode. Both IPv4 and
    // IPv6 tables are scanned: the transport binds 0.0.0.0, but a
    // holder may have bound the IPv6 wildcard instead.
    for table in ["/proc/net/udp", "/proc/net/udp6"] {
        let text = std::fs::read_to_string(table).ok()?;
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 {
                continue;
            }
            let Some((_, port_hex)) = fields[1].split_once(':') else {
                continue;
            };
            if u16::from_str_radix(port_hex, 16).ok() != Some(port) {
                continue;
            }
            // No state filter: a bound UDP socket shows `07`, a
            // connected one `01` — UDP has no listen state, and any
            // row with this local port holds it.
            return fields[9].parse::<u64>().ok();
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn pid_holding_socket(inode: u64) -> Option<u32> {
    let needle = format!("socket:[{inode}]");
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path())
                && target.to_string_lossy() == needle
            {
                return Some(pid);
            }
        }
    }
    None
}

#[cfg(windows)]
fn diagnose_windows(port: u16) -> Option<PortHolder> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedUdpTable, MIB_UDPROW_OWNER_PID, UDP_TABLE_OWNER_PID,
    };
    use windows::Win32::Networking::WinSock::AF_INET;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    // Ports in the UDP table are network byte order in the low 16 bits
    // (same layout as the TCP table rows — see the HIDMaestro
    // loopback-peer check).
    let table_port = |raw: u32| -> u16 { u16::try_from(raw & 0xFFFF).unwrap_or(0).swap_bytes() };
    let mut size: u32 = 0;
    // SAFETY: first call with a null table only queries the size
    // (`ERROR_INSUFFICIENT_BUFFER` is the expected outcome).
    let rc = unsafe {
        GetExtendedUdpTable(
            None,
            &raw mut size,
            false,
            u32::from(AF_INET.0),
            UDP_TABLE_OWNER_PID,
            0,
        )
    };
    if rc != 122 && rc != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` is a live byte buffer of the declared size; rows
    // are read back bounds-checked below.
    let rc = unsafe {
        GetExtendedUdpTable(
            Some(buf.as_mut_ptr().cast::<std::ffi::c_void>()),
            &raw mut size,
            false,
            u32::from(AF_INET.0),
            UDP_TABLE_OWNER_PID,
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let count = u32::from_ne_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let row_size = std::mem::size_of::<MIB_UDPROW_OWNER_PID>();
    for i in 0..count {
        let offset = 4 + i * row_size;
        if offset + row_size > buf.len() {
            break;
        }
        // SAFETY: bounds-checked above; the row is `#[repr(C)]` and
        // `Copy`, so an unaligned read is sound.
        let row: MIB_UDPROW_OWNER_PID =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(offset).cast()) };
        if table_port(row.dwLocalPort) != port {
            continue;
        }
        let pid = row.dwOwningPid;
        let exe_path = process_image_path(pid);
        let process_name = exe_path
            .as_deref()
            .map_or_else(|| format!("pid {pid}"), file_name_of)
            .to_string();
        return Some(PortHolder {
            pid,
            process_name,
            exe_path,
        });
    }
    None
}

#[cfg(windows)]
fn process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;
    // SAFETY: `OpenProcess` with a pid from the OS table; the handle
    // is closed below on every path.
    let handle: HANDLE =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` is a live wide-char buffer of `len` entries;
    // `QueryFullProcessImageNameW` writes at most `len` of them.
    let ok =
        unsafe { QueryFullProcessImageNameW(handle, 0, PWSTR(buf.as_mut_ptr()), &mut len).is_ok() };
    // SAFETY: the handle came from `OpenProcess` above and is closed
    // exactly once here.
    let _ = unsafe { CloseHandle(handle) };
    if !ok {
        return None;
    }
    String::from_utf16(&buf[..len as usize]).ok()
}

impl fmt::Display for PortHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    #[test]
    fn legacy_app_recognised_by_installed_and_dev_exe_names() {
        for name in [
            "RSH.exe",
            "rsh.exe",
            "C:\\Users\\Hammad\\AppData\\Local\\RSH\\RSH.exe",
        ] {
            let holder = PortHolder {
                pid: 1,
                process_name: name.to_string(),
                exe_path: None,
            };
            assert!(holder.is_legacy_rsh_app(), "{name} is the legacy app");
            assert!(!holder.is_current_rsh(), "{name} is not current RSH");
        }
        let dev = PortHolder {
            pid: 2,
            process_name: "rsh-app.exe".to_string(),
            exe_path: None,
        };
        assert!(dev.is_legacy_rsh_app(), "dev binary is the legacy app");
        let by_path = PortHolder {
            pid: 3,
            process_name: "unknown".to_string(),
            exe_path: Some("C:\\RSH\\RSH.exe".to_string()),
        };
        assert!(by_path.is_legacy_rsh_app(), "path match counts too");
    }

    #[test]
    fn current_stack_recognised_and_not_legacy() {
        for name in ["rsh-daemon", "rsh-daemon.exe", "rsh-ui.exe"] {
            let holder = PortHolder {
                pid: 1,
                process_name: name.to_string(),
                exe_path: None,
            };
            assert!(holder.is_current_rsh(), "{name} is current RSH");
            assert!(!holder.is_legacy_rsh_app(), "{name} is not legacy");
        }
        let other = PortHolder {
            pid: 1,
            process_name: "some-game.exe".to_string(),
            exe_path: None,
        };
        assert!(!other.is_legacy_rsh_app());
        assert!(!other.is_current_rsh());
    }

    #[test]
    fn describe_names_pid_and_path() {
        let holder = PortHolder {
            pid: 42,
            process_name: "RSH.exe".to_string(),
            exe_path: Some("C:\\RSH\\RSH.exe".to_string()),
        };
        assert_eq!(holder.describe(), "RSH.exe (pid 42) at C:\\RSH\\RSH.exe");
        let no_path = PortHolder {
            pid: 7,
            process_name: "RSH.exe".to_string(),
            exe_path: None,
        };
        assert_eq!(no_path.describe(), "RSH.exe (pid 7)");
    }

    /// Linux-only (the /proc lookup): a socket bound in this test
    /// process is found, with this process as the holder — the exact
    /// shape the daemon needs on the user PC.
    #[test]
    #[cfg(target_os = "linux")]
    fn diagnose_finds_own_bound_socket() {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("test socket binds");
        let port = socket.local_addr().expect("local addr").port();
        let holder = diagnose_udp_port_holder(port).expect("holder is found");
        assert_eq!(holder.pid, std::process::id(), "holder is this process");
        assert!(
            !holder.process_name.is_empty(),
            "holder has a name: {holder:?}"
        );
        assert!(
            holder
                .exe_path
                .as_ref()
                .is_some_and(|path| !path.is_empty()),
            "holder has an exe path: {holder:?}"
        );
    }

    /// A port nothing holds diagnoses as free — no false holder.
    #[test]
    #[cfg(target_os = "linux")]
    fn diagnose_free_port_is_none() {
        let probe = UdpSocket::bind("127.0.0.1:0").expect("probe binds");
        let port = probe.local_addr().expect("local addr").port();
        drop(probe);
        assert_eq!(diagnose_udp_port_holder(port), None, "free port, no holder");
    }
}
