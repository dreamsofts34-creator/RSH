//! Sidecar transports: stdio pipes and loopback TCP.
//!
//! Two transports carry the same JSON-lines framing:
//!
//! * **stdio** (Linux, tests, and non-elevated dev): the sidecar is a
//!   child process with piped stdin/stdout/stderr. EOF on stdout is
//!   immediate crash detection.
//! * **TCP loopback** (Windows production): the sidecar is elevated via
//!   `ShellExecuteEx` (`runas`), and `CreateProcess` handle redirection
//!   does not survive a `runas` launch — so the sidecar connects *back*
//!   to a `127.0.0.1` listener the Rust side opens, proving itself with
//!   the one-time token passed on its command line. Same framing, same
//!   JSON — only the byte pipe differs.
//!
//! Reads happen on a dedicated reader thread that forwards lines over
//! an [`std::sync::mpsc`] channel, so the manager can apply a reply
//! timeout instead of blocking forever on a hung sidecar. Stderr is
//! drained into a capped ring buffer for diagnostics (the sidecar logs
//! there — never stdout, which is the protocol).

use crate::protocol::bytes_to_hex;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Win32 `CREATE_NO_WINDOW` (`0x08000000`): suppress the console
/// window for a child process. Used by [`spawn_stdio`] (hotfix
/// v1.0.1, BUG #2 — belt-and-braces with the sidecar's `WinExe`
/// output type). The value is pinned by the `#[cfg(windows)]` test
/// below against the `windows` crate constant.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One event from the reader thread.
#[derive(Debug)]
pub enum TransportEvent {
    /// One complete line (without the trailing newline).
    Line(String),
    /// The stream ended (sidecar exited / closed the connection).
    Eof,
}

/// The write half of a live transport: send lines and kill the peer.
pub trait TransportWriter: Send {
    /// Write one line (newline added) and flush.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the sidecar is gone.
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;

    /// Best-effort terminate: close stdin / shut the socket down, then
    /// kill the child when there is one. Never panics.
    fn kill(&mut self);
}

/// A live sidecar connection: a writer, a line-event channel, and the
/// sidecar's recent stderr (diagnostics ring buffer, newest last,
/// capped at 64 lines).
pub struct LiveTransport {
    /// Send commands here.
    pub writer: Box<dyn TransportWriter>,
    /// Lines (and EOF) arrive here.
    pub events: Receiver<TransportEvent>,
    /// Last stderr lines from the sidecar (empty on the TCP path —
    /// an elevated sidecar's stderr is not capturable).
    pub stderr_lines: Arc<Mutex<VecDeque<String>>>,
}

/// Spawn the sidecar as a child with piped stdio.
///
/// `args` are passed verbatim (e.g. `--connect-back …` is never used
/// on this path). `extra_env` adds environment variables for the child
/// (stub fault injection in tests).
///
/// # Errors
///
/// Returns the spawn I/O error when the executable cannot be started.
pub fn spawn_stdio(
    path: &Path,
    args: &[String],
    extra_env: &[(String, String)],
) -> std::io::Result<LiveTransport> {
    let mut cmd = Command::new(path);
    cmd.args(args)
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Hotfix v1.0.1 (BUG #2, belt-and-braces with the sidecar's
    // `WinExe` output type): on Windows, never let a console-subsystem
    // sidecar binary flash a console window when spawned over stdio.
    // `CREATE_NO_WINDOW` only suppresses the window — the piped stdio
    // handles above are unaffected. The production `runas` path does
    // not go through here (`ShellExecuteEx` takes no creation flags),
    // so it relies on `WinExe` alone — see
    // `manager::spawn_elevated_runas`.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("piped stdin was not created"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("piped stdout was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("piped stderr was not created"))?;

    let stderr_lines: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    drain_to_ring(stderr, Arc::clone(&stderr_lines));

    let (tx, rx) = channel();
    spawn_line_reader(BufReader::new(stdout), tx);

    // The child's stdin + handle are shared with the writer thread
    // (H4): the thread owns the writes, the manager-side kill path
    // owns the terminate — either side can end the child, and killing
    // the child unblocks a writer thread stuck on a full pipe.
    let stdin_shared: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(Some(stdin)));
    let child_shared: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let inner: Box<dyn TransportWriter> = Box::new(StdioWriter {
        stdin: Arc::clone(&stdin_shared),
        child: Arc::clone(&child_shared),
    });
    let stdin_kill = Arc::clone(&stdin_shared);
    let child_kill = Arc::clone(&child_shared);
    let writer = queue_writer(
        inner,
        Box::new(move || kill_stdio(&stdin_kill, &child_kill)),
    );

    Ok(LiveTransport {
        writer,
        events: rx,
        stderr_lines,
    })
}

/// Close a stdio child's stdin (EOF → clean sidecar exit) and
/// terminate + reap the child. Idempotent: both handles are `take()`n.
fn kill_stdio(stdin: &Arc<Mutex<Option<ChildStdin>>>, child: &Arc<Mutex<Option<Child>>>) {
    // A poisoned mutex still holds the handle — recover it rather
    // than leaving a child alive because a writer thread panicked.
    let mut stdin_guard = stdin
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drop(stdin_guard.take());
    drop(stdin_guard);
    let mut child_guard = child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut child) = child_guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The write half of a stdio transport: the child's stdin plus the
/// child handle (for kill/reap). Both are shared with the manager-side
/// kill closure ([`kill_stdio`]) — see [`spawn_stdio`].
struct StdioWriter {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl TransportWriter for StdioWriter {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let mut guard = self
            .stdin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stdin) = guard.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "sidecar stdin is closed",
            ));
        };
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()
    }

    fn kill(&mut self) {
        kill_stdio(&self.stdin, &self.child);
    }
}

/// Open a loopback listener for the Windows elevated path. Returns the
/// listener and the ephemeral port to hand to the sidecar via
/// `--connect-back 127.0.0.1:<port>`.
/// The listener only ever accepts from loopback.
///
/// # Errors
///
/// Returns the bind I/O error when loopback cannot be bound.
pub fn listen_loopback() -> std::io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Accept one loopback connection within `timeout`, and wrap it as a
/// live transport. The peer must be the sidecar we just spawned; the
/// token check happens at the `hello` layer ([`crate::protocol`]).
///
/// L4: the peer address must be loopback — a non-loopback connection
/// is dropped and the wait continues (the listener only binds
/// `127.0.0.1`, so this is defence in depth). On Windows,
/// `expected_pid` (the spawned sidecar's PID) is additionally verified
/// against the TCP table ([`verify_loopback_peer_pid`]) — a foreign
/// local process that guesses the port + token window cannot pose as
/// the sidecar (C-TOK-1 / Security H-1).
///
/// # Errors
///
/// Returns [`std::io::ErrorKind::TimedOut`] when nobody connects in
/// time, or the accept I/O error.
pub fn accept_loopback(
    listener: &TcpListener,
    timeout: Duration,
    expected_pid: Option<u32>,
) -> std::io::Result<LiveTransport> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    let stream = loop {
        match listener.accept() {
            Ok((stream, addr)) => {
                // L4: loopback only.
                if !addr.ip().is_loopback() {
                    drop(stream);
                    continue;
                }
                #[cfg(windows)]
                if let Some(pid) = expected_pid {
                    verify_loopback_peer_pid(&stream, pid)?;
                }
                #[cfg(not(windows))]
                let _ = expected_pid;
                break stream;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "sidecar did not connect back in time",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e),
        }
    };
    stream.set_nodelay(true)?;
    let read_half = stream.try_clone()?;
    // The write half shares the socket for shutdown: killing the
    // transport shuts the socket down, which unblocks a writer thread
    // stuck on a hung sidecar (H4, TCP side).
    let peer = Arc::new(stream.try_clone()?);
    let (tx, rx) = channel();
    spawn_line_reader(BufReader::new(read_half), tx);
    let peer_kill = Arc::clone(&peer);
    let writer = queue_writer(
        Box::new(TcpWriter {
            stream: Some(BufWriter::new(stream)),
            peer,
        }),
        Box::new(move || {
            let _ = peer_kill.shutdown(std::net::Shutdown::Both);
        }),
    );
    Ok(LiveTransport {
        writer,
        events: rx,
        stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
    })
}

/// Windows-only (C-TOK-1): verify that the accepted loopback socket's
/// remote peer is the process `expected_pid` — via
/// `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_CONNECTIONS)`. Fail closed:
/// no matching row, or a different owner PID, refuses the connection.
#[cfg(windows)]
fn verify_loopback_peer_pid(stream: &TcpStream, expected_pid: u32) -> std::io::Result<()> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    // `127.0.0.1` as a little-endian u32, as the table stores it.
    const LOOPBACK_ADDR: u32 = 0x0100_007F;
    // `MIB_TCP_STATE_ESTABLISHED`.
    const ESTABLISHED: u32 = 5;

    let local_port = stream.local_addr()?.port();
    let remote_port = stream.peer_addr()?.port();
    // Ports in the TCP table are network byte order in the low 16 bits
    // (the low 16 bits of the stored u32, masked explicitly so the
    // truncation is intentional, not a silent `as` cast).
    let table_port = |raw: u32| -> u16 { u16::try_from(raw & 0xFFFF).unwrap_or(0).swap_bytes() };

    let mut size: u32 = 0;
    // SAFETY: first call with a null table only queries the size
    // (`ERROR_INSUFFICIENT_BUFFER` is the expected outcome).
    let rc = unsafe {
        GetExtendedTcpTable(
            None,
            &raw mut size,
            false,
            u32::from(AF_INET.0),
            TCP_TABLE_OWNER_PID_CONNECTIONS,
            0,
        )
    };
    if rc != 122 && rc != 0 {
        return Err(std::io::Error::other(format!(
            "GetExtendedTcpTable size query failed (Win32 {rc})"
        )));
    }
    if size == 0 {
        return Err(std::io::Error::other(
            "GetExtendedTcpTable returned an empty table",
        ));
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` is a live byte buffer of the declared size; the
    // table is read back through `MIB_TCPROW_OWNER_PID` rows below.
    let rc = unsafe {
        GetExtendedTcpTable(
            Some(buf.as_mut_ptr().cast::<std::ffi::c_void>()),
            &raw mut size,
            false,
            u32::from(AF_INET.0),
            TCP_TABLE_OWNER_PID_CONNECTIONS,
            0,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::other(format!(
            "GetExtendedTcpTable failed (Win32 {rc})"
        )));
    }
    let count = u32::from_ne_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    for i in 0..count {
        let offset = 4 + i * row_size;
        if offset + row_size > buf.len() {
            break;
        }
        // SAFETY: bounds-checked above; the row is `#[repr(C)]` and
        // `Copy`, so an unaligned read is sound.
        let row: MIB_TCPROW_OWNER_PID =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().add(offset).cast()) };
        if row.dwState == ESTABLISHED
            && table_port(row.dwLocalPort) == local_port
            && table_port(row.dwRemotePort) == remote_port
            && row.dwRemoteAddr == LOOPBACK_ADDR
        {
            if row.dwOwningPid == expected_pid {
                return Ok(());
            }
            return Err(std::io::Error::other(format!(
                "loopback peer is PID {}, not the spawned sidecar PID {expected_pid}",
                row.dwOwningPid
            )));
        }
    }
    Err(std::io::Error::other(
        "no TCP table row for the loopback peer — refusing the connection",
    ))
}

/// The write half of a TCP-loopback transport.
struct TcpWriter {
    stream: Option<BufWriter<TcpStream>>,
    /// A second handle to the same socket, used only to shut it down
    /// from the kill path (unblocks a writer thread stuck on a hung
    /// sidecar — H4, TCP side).
    peer: Arc<TcpStream>,
}

impl TransportWriter for TcpWriter {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "sidecar connection is closed",
            ));
        };
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()
    }

    fn kill(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream
                .into_inner()
                .map(|s| s.shutdown(std::net::Shutdown::Both));
        }
        let _ = self.peer.shutdown(std::net::Shutdown::Both);
    }
}

/// H4 — a dedicated writer thread + bounded queue, so the engine
/// thread never blocks on a hung sidecar's pipe.
///
/// Commands are queued (capacity [`WRITER_QUEUE_CAPACITY`]) and a
/// single writer thread drains them in order. `write_line` enqueues
/// without blocking: a full queue fails fast with `TimedOut` instead
/// of stalling the 200 Hz tick — the manager treats that as a sidecar
/// death and fails over. `kill` drops the queue and runs `on_kill`
/// (close stdin / shut the socket / terminate the child), which
/// unblocks a writer thread stuck mid-write; the thread then calls
/// the inner writer's `kill` (idempotent) and exits.
const WRITER_QUEUE_CAPACITY: usize = 4;

struct QueuedWriter {
    tx: Option<SyncSender<String>>,
    on_kill: Option<Box<dyn Fn() + Send>>,
}

impl TransportWriter for QueuedWriter {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(tx) = self.tx.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "sidecar writer is gone",
            ));
        };
        // Non-blocking enqueue (`send_timeout` is unstable on this
        // toolchain): a full queue means the sidecar stopped reading,
        // and the engine thread must fail fast — not wait — so the
        // manager can treat it as a sidecar death and fail over.
        tx.try_send(line.to_string()).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(_) => std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "sidecar write queue full — sidecar is not reading",
            ),
            std::sync::mpsc::TrySendError::Disconnected(_) => std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "sidecar writer thread is gone",
            ),
        })
    }

    fn kill(&mut self) {
        // Drop the queue first: the writer thread sees the disconnect
        // after its current write, then kills the inner writer.
        drop(self.tx.take());
        if let Some(on_kill) = self.on_kill.take() {
            on_kill();
        }
    }
}

fn queue_writer(
    inner: Box<dyn TransportWriter>,
    on_kill: Box<dyn Fn() + Send>,
) -> Box<dyn TransportWriter> {
    let (tx, rx) = sync_channel::<String>(WRITER_QUEUE_CAPACITY);
    std::thread::spawn(move || {
        let mut inner = inner;
        while let Ok(line) = rx.recv() {
            if inner.write_line(&line).is_err() {
                break;
            }
        }
        inner.kill();
    });
    Box::new(QueuedWriter {
        tx: Some(tx),
        on_kill: Some(on_kill),
    })
}

/// Maximum bytes of one sidecar line (protocol or stderr). A
/// compromised sidecar used to be able to exhaust RSH memory with a
/// single unbounded line (`BufRead::read_line` grows without limit);
/// anything longer is treated as a dead/malicious sidecar: the
/// protocol reader reports EOF, the stderr drain drops the line.
const MAX_LINE_LEN: usize = 64 * 1024;

/// Outcome of one capped line read.
enum CappedLine {
    /// One complete line, without its trailing newline.
    Line(String),
    /// Clean end of stream.
    Eof,
    /// Line longer than [`MAX_LINE_LEN`] (or invalid UTF-8): the
    /// caller decides (protocol: EOF; stderr: drop the line).
    Overlong,
}

/// Read one line, capped at [`MAX_LINE_LEN`] bytes. Reads at most
/// `MAX_LINE_LEN + 1` bytes: a line that fills the cap without
/// terminating is overlong, never partially delivered.
fn read_line_capped<R: BufRead>(reader: &mut R) -> CappedLine {
    let mut buf = Vec::new();
    let mut limited = reader.by_ref().take(MAX_LINE_LEN as u64 + 1);
    match limited.read_until(b'\n', &mut buf) {
        Ok(0) | Err(_) => CappedLine::Eof,
        Ok(_) => {
            if buf.len() > MAX_LINE_LEN {
                return CappedLine::Overlong;
            }
            match String::from_utf8(buf) {
                Ok(text) => CappedLine::Line(text.trim_end_matches(['\r', '\n']).to_string()),
                Err(_) => CappedLine::Overlong,
            }
        }
    }
}

/// Read lines on a thread and forward them; send [`TransportEvent::Eof`]
/// when the stream ends, then exit. A read error is reported as EOF
/// (the manager treats both as "sidecar died"). An overlong line is
/// also EOF: a sidecar that floods the protocol channel is dead to us.
fn spawn_line_reader<R: BufRead + Send + 'static>(mut reader: R, tx: Sender<TransportEvent>) {
    std::thread::spawn(move || {
        loop {
            match read_line_capped(&mut reader) {
                CappedLine::Line(text) => {
                    if tx.send(TransportEvent::Line(text)).is_err() {
                        return;
                    }
                }
                CappedLine::Eof | CappedLine::Overlong => {
                    let _ = tx.send(TransportEvent::Eof);
                    return;
                }
            }
        }
    });
}

/// Drain a `Read` (sidecar stderr) into a capped ring buffer on a
/// thread. Lines are kept newest-last; the buffer holds 64 lines.
/// Overlong lines are dropped, never stored — the ring cap alone does
/// not bound a single line's length.
fn drain_to_ring<R: Read + Send + 'static>(reader: R, ring: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            match read_line_capped(&mut reader) {
                CappedLine::Line(text) => {
                    if let Ok(mut guard) = ring.lock() {
                        guard.push_back(text);
                        while guard.len() > 64 {
                            guard.pop_front();
                        }
                    }
                }
                CappedLine::Eof => return,
                CappedLine::Overlong => {
                    // Drop the line and keep draining: stderr is
                    // diagnostics only, never the protocol.
                }
            }
        }
    });
}

/// Generate a one-time 128-bit hex token for the TCP connect-back.
///
/// 16 bytes from the OS CSPRNG ([`getrandom`]), hex-encoded. The
/// token proves the process that connects back is the sidecar we
/// spawned, not a stray local process — so it must be unpredictable,
/// not a time/pid xorshift (H1). The listener is still loopback-only
/// and short-lived; the token is one layer, not the whole boundary.
///
/// # Errors
///
/// Returns the OS RNG error instead of panicking when entropy is
/// unavailable — the caller (Windows elevated spawn) surfaces it as
/// a connect failure via `HidMaestroError::Io` instead of crashing
/// the engine thread (DS-42).
pub fn generate_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes_to_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process that exits immediately with no output, for the stdio
    /// EOF test: `/bin/true` on Unix; `cmd /c exit 0` on Windows
    /// (`cmd` resolves via PATH — `Command::new` does the lookup
    /// there). The Unix paths do not exist on Windows, so the tests
    /// that spawned them failed there with `NotFound` (dry run #7).
    #[cfg(unix)]
    fn immediate_exit_helper() -> (std::path::PathBuf, Vec<String>) {
        (std::path::PathBuf::from("/bin/true"), Vec::new())
    }
    #[cfg(windows)]
    fn immediate_exit_helper() -> (std::path::PathBuf, Vec<String>) {
        (
            std::path::PathBuf::from("cmd"),
            vec!["/c".to_string(), "exit".to_string(), "0".to_string()],
        )
    }

    /// A process that reads stdin but never replies, for the
    /// queued-writer test: `/bin/cat` on Unix; bare `cmd` on Windows
    /// (reads commands from stdin until it closes, and the test
    /// sends nothing). Dropped like `cat`, the transport ends it.
    #[cfg(unix)]
    fn stdin_sink_helper() -> (std::path::PathBuf, Vec<String>) {
        (std::path::PathBuf::from("/bin/cat"), Vec::new())
    }
    #[cfg(windows)]
    fn stdin_sink_helper() -> (std::path::PathBuf, Vec<String>) {
        (std::path::PathBuf::from("cmd"), Vec::new())
    }

    #[test]
    fn token_is_32_hex_and_unique() {
        let a = generate_token().expect("OS RNG available in tests");
        let b = generate_token().expect("OS RNG available in tests");
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// H1: a line longer than 64 KiB is EOF, never delivered — a
    /// compromised sidecar cannot grow RSH memory without bound.
    #[test]
    fn overlong_line_is_eof_not_delivered() {
        let mut data = vec![b'x'; 64 * 1024 + 100];
        data.push(b'\n');
        let (tx, rx) = channel();
        spawn_line_reader(BufReader::new(std::io::Cursor::new(data)), tx);
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(TransportEvent::Eof) => {}
            other => panic!("expected EOF for overlong line, got {other:?}"),
        }
    }

    /// H1: a line exactly at the cap still flows; the cap is a
    /// flood defence, not a normal-line limit.
    #[test]
    fn line_at_cap_is_delivered() {
        let mut data = vec![b'y'; 64 * 1024 - 1];
        data.push(b'\n');
        let (tx, rx) = channel();
        spawn_line_reader(BufReader::new(std::io::Cursor::new(data)), tx);
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(TransportEvent::Line(text)) => assert_eq!(text.len(), 64 * 1024 - 1),
            other => panic!("expected the capped line, got {other:?}"),
        }
    }

    #[test]
    fn stdio_spawn_missing_exe_fails() {
        let result = spawn_stdio(Path::new("/nonexistent/rsh-hm-sidecar"), &[], &[]);
        assert!(result.is_err());
    }

    /// Hotfix v1.0.1: the `CREATE_NO_WINDOW` value passed to the
    /// stdio spawn must be the real Win32 flag — a typo here would
    /// silently spawn a console-subsystem sidecar with a visible
    /// console window. Windows-only (the `windows` crate does not
    /// compile on Linux); runs on Windows CI.
    #[cfg(windows)]
    #[test]
    fn create_no_window_matches_win32_constant() {
        assert_eq!(
            CREATE_NO_WINDOW,
            windows::Win32::System::Threading::CREATE_NO_WINDOW.0
        );
    }

    #[test]
    fn stdio_spawn_and_eof_detection() {
        // The helper exits immediately with no output: the reader
        // must report EOF promptly.
        let (path, args) = immediate_exit_helper();
        let transport = spawn_stdio(&path, &args, &[]).unwrap();
        match transport.events.recv_timeout(Duration::from_secs(5)) {
            Ok(TransportEvent::Eof) => {}
            other => panic!("expected EOF, got {other:?}"),
        }
    }

    #[test]
    fn loopback_listen_binds_ephemeral_port() {
        let (listener, port) = listen_loopback().unwrap();
        assert_ne!(port, 0);
        drop(listener);
    }

    #[test]
    fn loopback_accept_times_out_without_peer() {
        let (listener, _port) = listen_loopback().unwrap();
        match accept_loopback(&listener, Duration::from_millis(50), None) {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::TimedOut),
            Ok(_) => panic!("expected a timeout, got a connection"),
        }
    }

    /// L4: a non-loopback peer cannot be tested from this process
    /// (the listener binds 127.0.0.1), but a loopback peer IS
    /// accepted — the happy path the PID check builds on.
    #[test]
    fn loopback_accept_accepts_loopback_peer() {
        let (listener, port) = listen_loopback().unwrap();
        let handle =
            std::thread::spawn(move || std::net::TcpStream::connect(("127.0.0.1", port)).unwrap());
        let transport = accept_loopback(&listener, Duration::from_secs(5), None).unwrap();
        let _peer = handle.join().unwrap();
        drop(transport);
    }

    /// H4: the writer thread + bounded queue accept lines without
    /// blocking the caller, even when the reader never reads.
    #[test]
    fn queued_writer_does_not_block_on_unread_pipe() {
        // The helper reads stdin but never replies — the queue must
        // still accept lines (the reader thread is irrelevant here).
        let (path, args) = stdin_sink_helper();
        let transport = spawn_stdio(&path, &args, &[]).unwrap();
        drop(transport);
    }
}
