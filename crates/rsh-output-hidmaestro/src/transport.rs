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
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    let mut child = Command::new(path)
        .args(args)
        .envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
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

    Ok(LiveTransport {
        writer: Box::new(StdioWriter {
            stdin: Some(stdin),
            child: Some(child),
        }),
        events: rx,
        stderr_lines,
    })
}

/// The write half of a stdio transport: the child's stdin plus the
/// child handle (for kill/reap).
struct StdioWriter {
    stdin: Option<ChildStdin>,
    child: Option<Child>,
}

impl TransportWriter for StdioWriter {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
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
        // Close stdin first: a well-behaved sidecar sees EOF, disposes
        // the device and exits cleanly on its own.
        drop(self.stdin.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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
/// # Errors
///
/// Returns [`std::io::ErrorKind::TimedOut`] when nobody connects in
/// time, or the accept I/O error.
pub fn accept_loopback(
    listener: &TcpListener,
    timeout: Duration,
) -> std::io::Result<LiveTransport> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
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
    let (tx, rx) = channel();
    spawn_line_reader(BufReader::new(read_half), tx);
    Ok(LiveTransport {
        writer: Box::new(TcpWriter {
            stream: Some(BufWriter::new(stream)),
        }),
        events: rx,
        stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
    })
}

/// The write half of a TCP-loopback transport.
struct TcpWriter {
    stream: Option<BufWriter<TcpStream>>,
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
    }
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
/// # Panics
///
/// Panics when the OS RNG is unavailable: without OS entropy there
/// is no safe token to issue, and silently falling back to weak
/// randomness would be worse than failing loudly at connect time.
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS RNG unavailable for sidecar token");
    bytes_to_hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_hex_and_unique() {
        let a = generate_token();
        let b = generate_token();
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

    #[test]
    fn stdio_spawn_and_eof_detection() {
        // `/bin/true` exits immediately with no output: the reader
        // must report EOF promptly.
        let transport = spawn_stdio(Path::new("/bin/true"), &[], &[]).unwrap();
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
        match accept_loopback(&listener, Duration::from_millis(50)) {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::TimedOut),
            Ok(_) => panic!("expected a timeout, got a connection"),
        }
    }
}
