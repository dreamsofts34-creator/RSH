//! UDP transport for protocol v2.1 (Phase C1 / SW-R5.1).
//!
//! One [`std::net::UdpSocket`] bound to the local stream port (4211 by
//! default) serves all three UDP channels:
//!
//! * HELO (PC → board, port 4210) every second — the 1 Hz keep-alive;
//! * stream (board → PC, port 4211) — `"RSH2"` frames for the engine's
//!   [`PacketSource`], plus `"RSHR"` HELLO replies and `0xA6` status
//!   packets, demuxed by magic;
//! * control (PC ↔ board, port 4212) — raw `"RSHC"` frames for the
//!   [`ControlTransport`] trait (HMAC verification lives in the control
//!   client, not here).
//!
//! The socket is a `std` socket, not `tokio::net::UdpSocket`: the
//! workspace tokio pin does not enable the `net` feature, and this crate
//! may not edit `Cargo.toml`. A blocking reader thread forwards parsed
//! frames into bounded tokio `mpsc` channels; [`PacketSource::next_frame`]
//! awaits the channel, which is cancellation-safe, so the engine's
//! zero-timeout polls never lose a frame. Full channels drop the newest
//! frame and count it (`dropped_overflow`) instead of blocking the
//! reader — the engine keeps only the latest frame per tick anyway.
//!
//! Every datagram from an unexpected sender IP is dropped and counted
//! (spoof guard). The board sends from different source ports per
//! channel (4210 HELLO, 4211 stream, 4212 control), so the filter
//! compares the sender **IP only**. The expected board address is
//! refreshable at runtime with [`UdpTransport::set_expected_remote`] —
//! the C# v2.0.4 lesson: the filter must follow the configured IP, or a
//! changed IP silently drops every frame.

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

use rsh_engine::engine::PacketSource;
use rsh_protocol::{
    DatagramKind, HelloReply, HeloRequest, StatusPacket, StreamError, StreamFrame,
    classify_datagram,
};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::{ControlTransport, TransportError};

/// Board HELO port (PC → board, 1 Hz keep-alive).
pub const HELO_PORT: u16 = 4210;
/// Local stream port (board → PC, 200 Hz stream + HELLO replies).
pub const STREAM_PORT: u16 = 4211;
/// Board control port (PC ↔ board, HMAC control frames).
pub const CONTROL_PORT: u16 = 4212;
/// Default board IP on its `SoftAP` (`D33` fallback).
pub const DEFAULT_BOARD_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 4, 1);
/// HELO send interval: 1 Hz.
pub const HELO_INTERVAL: Duration = Duration::from_secs(1);
/// Largest datagram this transport reads (control frames cap at 1053 B).
pub const MAX_DATAGRAM_LEN: usize = 2048;
/// Stream-frame channel capacity (200 Hz; the engine drains each tick).
const FRAME_CHANNEL_CAPACITY: usize = 256;
/// Control-reply channel capacity (at most one request in flight).
const CONTROL_CHANNEL_CAPACITY: usize = 16;
/// HELLO-reply channel capacity.
const HELLO_CHANNEL_CAPACITY: usize = 8;
/// Status-packet channel capacity.
const STATUS_CHANNEL_CAPACITY: usize = 16;
/// Reader-thread `recv` timeout: how promptly shutdown is noticed.
const READER_POLL_TIMEOUT: Duration = Duration::from_millis(100);
/// HELO-thread sleep granularity while waiting for the next HELO.
const HELO_SLEEP_STEP: Duration = Duration::from_millis(50);

/// Live transport counters, shared with the background threads.
#[derive(Debug, Default)]
struct AtomicCounters {
    /// Datagrams received on the socket (any sender, any shape).
    datagrams_received: AtomicU64,
    /// Datagrams dropped for any reason (sum of the `dropped_*` below).
    datagrams_dropped: AtomicU64,
    /// Datagrams dropped because the sender IP was not the board.
    dropped_wrong_sender: AtomicU64,
    /// Stream datagrams dropped with a CRC mismatch.
    dropped_bad_crc: AtomicU64,
    /// Datagrams dropped as malformed (bad magic, length, version, …).
    dropped_malformed: AtomicU64,
    /// Frames dropped because a channel was full.
    dropped_overflow: AtomicU64,
    /// `recv` I/O errors (socket-level, not datagram-level).
    receive_errors: AtomicU64,
    /// HELO/control `send` I/O errors.
    send_errors: AtomicU64,
    /// HELO keep-alives sent.
    helos_sent: AtomicU64,
    /// Stream frames parsed and handed to the frame channel.
    frames_delivered: AtomicU64,
    /// Raw control replies received from the board.
    control_received: AtomicU64,
    /// HELLO replies parsed and handed to the hello channel.
    hello_received: AtomicU64,
    /// Status packets parsed and handed to the status channel.
    status_received: AtomicU64,
    /// Reader-thread panics caught by the `catch_unwind` guard (M6:
    /// a panicking reader must be visible in `counters()`, not silent).
    reader_panics: AtomicU64,
    /// HELO-thread panics caught by the `catch_unwind` guard (M6).
    helo_panics: AtomicU64,
}

impl AtomicCounters {
    /// Snapshot the live counters.
    fn snapshot(&self) -> UdpCounters {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        UdpCounters {
            datagrams_received: load(&self.datagrams_received),
            datagrams_dropped: load(&self.datagrams_dropped),
            dropped_wrong_sender: load(&self.dropped_wrong_sender),
            dropped_bad_crc: load(&self.dropped_bad_crc),
            dropped_malformed: load(&self.dropped_malformed),
            dropped_overflow: load(&self.dropped_overflow),
            receive_errors: load(&self.receive_errors),
            send_errors: load(&self.send_errors),
            helos_sent: load(&self.helos_sent),
            frames_delivered: load(&self.frames_delivered),
            control_received: load(&self.control_received),
            hello_received: load(&self.hello_received),
            status_received: load(&self.status_received),
            reader_panics: load(&self.reader_panics),
            helo_panics: load(&self.helo_panics),
        }
    }
}

/// Snapshot of the [`UdpTransport`] counters (see `AtomicCounters`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UdpCounters {
    /// Datagrams received on the socket (any sender, any shape).
    pub datagrams_received: u64,
    /// Datagrams dropped for any reason (sum of the `dropped_*` below).
    pub datagrams_dropped: u64,
    /// Datagrams dropped because the sender IP was not the board.
    pub dropped_wrong_sender: u64,
    /// Stream datagrams dropped with a CRC mismatch.
    pub dropped_bad_crc: u64,
    /// Datagrams dropped as malformed (bad magic, length, version, …).
    pub dropped_malformed: u64,
    /// Frames dropped because a channel was full.
    pub dropped_overflow: u64,
    /// `recv` I/O errors (socket-level, not datagram-level).
    pub receive_errors: u64,
    /// HELO/control `send` I/O errors.
    pub send_errors: u64,
    /// HELO keep-alives sent.
    pub helos_sent: u64,
    /// Stream frames parsed and handed to the frame channel.
    pub frames_delivered: u64,
    /// Raw control replies received from the board.
    pub control_received: u64,
    /// HELLO replies parsed and handed to the hello channel.
    pub hello_received: u64,
    /// Status packets parsed and handed to the status channel.
    pub status_received: u64,
    /// Reader-thread panics caught by the `catch_unwind` guard (M6).
    pub reader_panics: u64,
    /// HELO-thread panics caught by the `catch_unwind` guard (M6).
    pub helo_panics: u64,
}

/// Configuration for [`UdpTransport::bind`].
#[derive(Clone, Copy, Debug)]
pub struct UdpConfig {
    /// Board IP (filter + HELO/control target host).
    pub board_ip: IpAddr,
    /// Board HELO port (default [`HELO_PORT`]).
    pub helo_port: u16,
    /// Board control port (default [`CONTROL_PORT`]).
    pub control_port: u16,
    /// Local port to bind for the stream (default [`STREAM_PORT`]);
    /// `0` binds an ephemeral port (used by tests).
    pub local_port: u16,
    /// Local IP to bind (default `0.0.0.0`).
    pub bind_ip: IpAddr,
    /// HELO send interval (default [`HELO_INTERVAL`]).
    pub helo_interval: Duration,
    /// HELO `sessionFlag`: `0` resume, `1` new session (default `0`).
    pub session_flag: u8,
}

impl UdpConfig {
    /// Build a config for the board at `board_ip`, with the standard ports.
    #[must_use]
    pub const fn new(board_ip: IpAddr) -> Self {
        Self {
            board_ip,
            helo_port: HELO_PORT,
            control_port: CONTROL_PORT,
            local_port: STREAM_PORT,
            bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            helo_interval: HELO_INTERVAL,
            session_flag: 0,
        }
    }
}

impl Default for UdpConfig {
    /// Default config: board at [`DEFAULT_BOARD_IP`], standard ports.
    fn default() -> Self {
        Self::new(IpAddr::V4(DEFAULT_BOARD_IP))
    }
}

/// Channel senders the reader thread forwards demuxed datagrams into.
struct DemuxSenders {
    /// Decoded stream frames.
    frame: mpsc::Sender<StreamFrame>,
    /// Raw control replies.
    control: mpsc::Sender<Vec<u8>>,
    /// Decoded HELLO replies.
    hello: mpsc::Sender<HelloReply>,
    /// Decoded status packets.
    status: mpsc::Sender<StatusPacket>,
}

/// UDP transport: HELO sender, stream receiver, control client (C1).
///
/// Owns one bound socket plus a reader thread and a HELO thread; both
/// stop promptly after [`Self::shutdown`] or when the transport is dropped.
#[derive(Debug)]
pub struct UdpTransport {
    /// The single bound socket (stream port; also sends HELO/control).
    socket: Arc<UdpSocket>,
    /// Expected board address (HELO target); its IP is the sender filter.
    expected_remote: Arc<RwLock<SocketAddr>>,
    /// Board control port (target port for control frames).
    control_port: u16,
    /// Stream frames for [`PacketSource::next_frame`].
    frame_rx: mpsc::Receiver<StreamFrame>,
    /// Raw control replies for [`ControlTransport::recv_control`].
    control_rx: mpsc::Receiver<Vec<u8>>,
    /// HELLO replies, in arrival order.
    hello_rx: mpsc::Receiver<HelloReply>,
    /// Status packets, in arrival order.
    status_rx: mpsc::Receiver<StatusPacket>,
    /// Live counters (shared with the reader/HELO threads).
    counters: Arc<AtomicCounters>,
    /// Set by [`Self::shutdown`] / [`Drop`]; the threads exit promptly.
    shutdown: Arc<AtomicBool>,
    /// Reader-thread handle (M6): polled by [`Self::reader_alive`]
    /// so a dead reader is observable instead of silent. The handle
    /// is never joined — the thread is detached and exits on
    /// shutdown; joining on drop could block the caller.
    reader_handle: Option<JoinHandle<()>>,
    /// HELO-thread handle (M6), same contract as `reader_handle`.
    helo_handle: Option<JoinHandle<()>>,
}

/// Typed UDP bind failure (C-12): the `io::ErrorKind` behind a
/// failed [`UdpTransport::bind`], classified so the UI can hint
/// instead of showing a raw OS string. Maps 1:1 onto
/// `rsh_api::BindErrorKind` at the app-core boundary.
#[derive(Debug)]
pub enum UdpBindError {
    /// Local port already in use (another RSH, or a stale socket).
    AddrInUse,
    /// Bind denied (missing privilege / restricted interface).
    PermissionDenied,
    /// No such interface / address not available on this host.
    NoInterface,
    /// Any other I/O failure (the error is carried).
    Other(std::io::Error),
}

impl UdpBindError {
    /// Classify an `io::Error` by kind.
    #[must_use]
    pub fn from_io(error: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        match error.kind() {
            ErrorKind::AddrInUse => Self::AddrInUse,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::AddrNotAvailable => Self::NoInterface,
            _ => Self::Other(std::io::Error::new(error.kind(), error.to_string())),
        }
    }

    /// Short stable code for logs.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AddrInUse => "AddrInUse",
            Self::PermissionDenied => "PermissionDenied",
            Self::NoInterface => "NoInterface",
            Self::Other(_) => "BindFailed",
        }
    }

    /// The `io::ErrorKind` to report when downgrading to a plain
    /// [`TransportError::Io`](crate::TransportError::Io).
    #[must_use]
    pub fn io_kind(&self) -> std::io::ErrorKind {
        match self {
            Self::AddrInUse => std::io::ErrorKind::AddrInUse,
            Self::PermissionDenied => std::io::ErrorKind::PermissionDenied,
            Self::NoInterface => std::io::ErrorKind::AddrNotAvailable,
            Self::Other(error) => error.kind(),
        }
    }
}

impl std::fmt::Display for UdpBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Other(error) => write!(f, "UDP bind failed: {error}"),
            _ => write!(f, "UDP bind failed: {}", self.code()),
        }
    }
}

impl std::error::Error for UdpBindError {}

impl From<std::io::Error> for UdpBindError {
    fn from(error: std::io::Error) -> Self {
        Self::from_io(&error)
    }
}

impl UdpTransport {
    /// Bind the local stream port and start the reader + HELO threads.
    ///
    /// The first HELO is sent immediately, then one every
    /// `config.helo_interval`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the local socket cannot be
    /// bound or a background thread cannot be spawned.
    pub fn bind(config: UdpConfig) -> Result<Self, TransportError> {
        Self::bind_typed(config).map_err(|error| match error {
            UdpBindError::Other(io) => TransportError::Io(io),
            other => TransportError::Io(std::io::Error::new(
                other.io_kind(),
                format!("UDP bind failed: {}", other.code()),
            )),
        })
    }

    /// Bind like [`Self::bind`], but with a typed failure (C-12):
    /// the caller learns *why* the bind failed (`AddrInUse` /
    /// `PermissionDenied` / `NoInterface`) instead of dropping the
    /// error with `.ok()`. The [`UdpBindError`] maps 1:1 onto
    /// `rsh_api::BindErrorKind` at the app-core boundary.
    ///
    /// # Errors
    ///
    /// Returns [`UdpBindError`] when the local socket cannot be
    /// bound or a background thread cannot be spawned.
    pub fn bind_typed(config: UdpConfig) -> Result<Self, UdpBindError> {
        let socket = Arc::new(
            UdpSocket::bind(SocketAddr::new(config.bind_ip, config.local_port))
                .map_err(UdpBindError::from)?,
        );
        let expected_remote = Arc::new(RwLock::new(SocketAddr::new(
            config.board_ip,
            config.helo_port,
        )));
        let counters = Arc::new(AtomicCounters::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let (frame_tx, frame_rx) = mpsc::channel(FRAME_CHANNEL_CAPACITY);
        let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
        let (hello_tx, hello_rx) = mpsc::channel(HELLO_CHANNEL_CAPACITY);
        let (status_tx, status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
        let senders = DemuxSenders {
            frame: frame_tx,
            control: control_tx,
            hello: hello_tx,
            status: status_tx,
        };
        let mut reader_handle = None;
        let mut helo_handle = None;
        spawn_reader(
            Arc::clone(&socket),
            Arc::clone(&expected_remote),
            senders,
            Arc::clone(&counters),
            Arc::clone(&shutdown),
        )
        .map(|handle| reader_handle = Some(handle))?;
        spawn_helo(
            Arc::clone(&socket),
            Arc::clone(&expected_remote),
            config.session_flag,
            config.helo_interval,
            Arc::clone(&counters),
            Arc::clone(&shutdown),
        )
        .map(|handle| helo_handle = Some(handle))?;
        Ok(Self {
            socket,
            expected_remote,
            control_port: config.control_port,
            frame_rx,
            control_rx,
            hello_rx,
            status_rx,
            counters,
            shutdown,
            reader_handle,
            helo_handle,
        })
    }

    /// Refresh the expected board address (filter + HELO/control target).
    ///
    /// The sender filter, the HELO target and the control target all read
    /// this address on every use, so a refresh takes effect immediately
    /// for traffic already in flight — the C# v2.0.4 regression was a
    /// filter that was never refreshed after the configured IP changed.
    /// The port component is the board's HELO port; the control target
    /// keeps the configured control port on the new IP.
    pub fn set_expected_remote(&self, addr: SocketAddr) {
        match self.expected_remote.write() {
            Ok(mut guard) => *guard = addr,
            Err(poisoned) => *poisoned.into_inner() = addr,
        }
    }

    /// Current expected board address.
    #[must_use]
    pub fn expected_remote(&self) -> SocketAddr {
        read_expected_addr(&self.expected_remote)
    }

    /// Send one HELO request to the board's HELO port (protocol §2.1).
    ///
    /// One-shot, for the session layer: a new session needs a fresh
    /// nonce (D30 — routine flag-0 HELOs return the old nonce, and the
    /// old nonce/counter are never reused after a session ends), and
    /// only a HELO with `session_flag 1` gets one. The 1 Hz HELO thread
    /// always sends flag 0 (resume), so the session activator calls
    /// this at activation start (and once on retry); steady-state
    /// HELOs are untouched. A send failure is reported, never retried
    /// here — the caller treats it as "no HELLO will arrive" and times
    /// out instead of failing the transport (in sandboxes `sendto` is
    /// blocked while loopback receive still works).
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the datagram cannot be sent.
    pub fn send_helo(&self, session_flag: u8) -> Result<(), TransportError> {
        let helo = HeloRequest::new_v2(session_flag).encode();
        self.socket
            .send_to(&helo, self.board_helo_addr())
            .map(|_| ())
            .map_err(TransportError::Io)
    }

    /// Board HELO target (the expected remote address itself).
    #[must_use]
    pub fn board_helo_addr(&self) -> SocketAddr {
        self.expected_remote()
    }

    /// Board control target (expected board IP, configured control port).
    #[must_use]
    pub fn board_control_addr(&self) -> SocketAddr {
        SocketAddr::new(self.expected_remote().ip(), self.control_port)
    }

    /// Local address the socket is bound to.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the socket reports no local
    /// address (only possible after the socket is closed).
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.socket.local_addr().map_err(TransportError::Io)
    }

    /// Snapshot of the live counters.
    #[must_use]
    pub fn counters(&self) -> UdpCounters {
        self.counters.snapshot()
    }

    /// Stop the reader + HELO threads. Idempotent; [`Drop`] calls this.
    ///
    /// L11: besides setting the flag, the socket is poked with an
    /// empty datagram to its own bound address — a reader blocked in
    /// `recv_from` wakes immediately instead of waiting out its
    /// 100 ms read timeout. (`std::net::UdpSocket` has no
    /// `shutdown()`/`close()`; the poke is the std-only equivalent.
    /// The reader checks the flag before counting, so the poke never
    /// pollutes the counters. If the poke fails, the 100 ms read
    /// timeout still bounds the exit.)
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Ok(addr) = self.socket.local_addr() {
            let _ = self.socket.send_to(&[], addr);
        }
    }

    /// `true` while the reader thread is still running (M6 health
    /// check — poll this alongside `counters().reader_panics`).
    #[must_use]
    pub fn reader_alive(&self) -> bool {
        self.reader_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// `true` while the HELO thread is still running (M6).
    #[must_use]
    pub fn helo_alive(&self) -> bool {
        self.helo_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// Await the next HELLO reply. Returns [`None`] once the transport
    /// is shut down.
    pub fn next_hello(&mut self) -> impl Future<Output = Option<HelloReply>> + Send {
        self.hello_rx.recv()
    }

    /// Await the next status packet. Returns [`None`] once the transport
    /// is shut down.
    pub fn next_status(&mut self) -> impl Future<Output = Option<StatusPacket>> + Send {
        self.status_rx.recv()
    }
}

impl Drop for UdpTransport {
    /// Signal the background threads to exit; they notice within ~100 ms.
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PacketSource for UdpTransport {
    /// Await the next parsed stream frame. Malformed datagrams never
    /// reach here (dropped + counted by the reader thread); [`None`]
    /// means the transport shut down.
    fn next_frame(&mut self) -> impl Future<Output = Option<StreamFrame>> + Send {
        self.frame_rx.recv()
    }

    /// Non-blocking drain of one queued status packet (H1 condition
    /// 12: the status pump — the forwarder drains this after every
    /// stream frame so board loss counters and `btnsEx` reach the
    /// engine instead of sitting in this channel).
    fn try_next_status(&mut self) -> Option<StatusPacket> {
        self.status_rx.try_recv().ok()
    }
}

impl ControlTransport for UdpTransport {
    /// Send one raw control frame to the board's control port.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Io`] when the datagram cannot be sent.
    fn send_control(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), TransportError>> + Send {
        let socket = Arc::clone(&self.socket);
        let target = self.board_control_addr();
        let owned = bytes.to_vec();
        async move {
            socket
                .send_to(&owned, target)
                .map(|_| ())
                .map_err(TransportError::Io)
        }
    }

    /// Receive one raw control reply, waiting at most `timeout_duration`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Timeout`] when no reply arrives in time,
    /// and [`TransportError::Closed`] when the transport shut down.
    async fn recv_control(
        &mut self,
        timeout_duration: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        match timeout(timeout_duration, self.control_rx.recv()).await {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(TransportError::Closed),
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

/// Read the expected board address, tolerating a poisoned lock.
fn read_expected_addr(expected: &RwLock<SocketAddr>) -> SocketAddr {
    match expected.read() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Spawn the reader thread (detached; it exits on shutdown).
/// Returns the thread handle so the transport can poll its health
/// (M6). A reader panic is caught and counted in `reader_panics`
/// instead of silently killing the stream.
fn spawn_reader(
    socket: Arc<UdpSocket>,
    expected: Arc<RwLock<SocketAddr>>,
    senders: DemuxSenders,
    counters: Arc<AtomicCounters>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    Builder::new()
        .name("rsh-udp-reader".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                reader_loop(&socket, &expected, &senders, &counters, &shutdown);
            }));
            if result.is_err() {
                counters.reader_panics.fetch_add(1, Ordering::Relaxed);
            }
        })
}

/// The reader loop: `recv`, sender-filter, demux by magic, parse, forward.
fn reader_loop(
    socket: &UdpSocket,
    expected: &RwLock<SocketAddr>,
    senders: &DemuxSenders,
    counters: &AtomicCounters,
    shutdown: &AtomicBool,
) {
    // A read timeout keeps shutdown responsive; if the platform refuses
    // it, fall back to non-blocking polls with a 1 ms sleep.
    let polling = socket.set_read_timeout(Some(READER_POLL_TIMEOUT)).is_err()
        && socket.set_nonblocking(true).is_ok();
    let mut buf = [0u8; MAX_DATAGRAM_LEN];
    while !shutdown.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                // L11: the shutdown poke (an empty datagram to our own
                // address) wakes recv — exit before counting it.
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                counters.datagrams_received.fetch_add(1, Ordering::Relaxed);
                if src.ip() != read_expected_addr(expected).ip() {
                    counters
                        .dropped_wrong_sender
                        .fetch_add(1, Ordering::Relaxed);
                    counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                handle_datagram(&buf[..len], senders, counters);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                if polling {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            Err(_) => {
                counters.receive_errors.fetch_add(1, Ordering::Relaxed);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Demux one accepted datagram by magic, parse it, and forward it — or
/// drop it and count the reason. Never panics on untrusted input.
fn handle_datagram(data: &[u8], senders: &DemuxSenders, counters: &AtomicCounters) {
    if data.starts_with(b"RSH2") {
        match StreamFrame::parse(data) {
            Ok(frame) => {
                // L10: delivered means it actually reached the
                // channel — a full channel drops the frame, and the
                // counter must say so (it used to count first, then
                // drop silently on overflow).
                if senders.frame.try_send(frame).is_ok() {
                    counters.frames_delivered.fetch_add(1, Ordering::Relaxed);
                } else {
                    count_overflow(counters);
                }
            }
            Err(StreamError::BadCrc) => {
                counters.dropped_bad_crc.fetch_add(1, Ordering::Relaxed);
                counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
                counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    } else if data.starts_with(b"RSHR") {
        if let Ok(reply) = HelloReply::parse(data) {
            counters.hello_received.fetch_add(1, Ordering::Relaxed);
            if senders.hello.try_send(reply).is_err() {
                count_overflow(counters);
            }
        } else {
            counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
        }
    } else if data.starts_with(b"RSHC") {
        // Control replies are raw bytes here; HMAC verification belongs
        // to the control client, which matches them by `reqId`.
        counters.control_received.fetch_add(1, Ordering::Relaxed);
        if senders.control.try_send(data.to_vec()).is_err() {
            count_overflow(counters);
        }
    } else if classify_datagram(data) == DatagramKind::Status {
        if let Ok(packet) = StatusPacket::parse(data) {
            counters.status_received.fetch_add(1, Ordering::Relaxed);
            if senders.status.try_send(packet).is_err() {
                count_overflow(counters);
            }
        } else {
            counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
            counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        // Legacy v1, unknown magic, or truncated: dropped, counted.
        counters.dropped_malformed.fetch_add(1, Ordering::Relaxed);
        counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count a frame lost because its channel was full.
fn count_overflow(counters: &AtomicCounters) {
    counters.dropped_overflow.fetch_add(1, Ordering::Relaxed);
    counters.datagrams_dropped.fetch_add(1, Ordering::Relaxed);
}

/// Spawn the HELO thread (detached; it exits on shutdown).
/// Returns the thread handle (M6); a HELO panic is caught and
/// counted in `helo_panics`.
fn spawn_helo(
    socket: Arc<UdpSocket>,
    expected: Arc<RwLock<SocketAddr>>,
    session_flag: u8,
    interval: Duration,
    counters: Arc<AtomicCounters>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    Builder::new()
        .name("rsh-udp-helo".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                helo_loop(
                    &socket,
                    &expected,
                    session_flag,
                    interval,
                    &counters,
                    &shutdown,
                );
            }));
            if result.is_err() {
                counters.helo_panics.fetch_add(1, Ordering::Relaxed);
            }
        })
}

/// The HELO loop: send immediately, then once per `interval`, reading the
/// (refreshable) expected address before every send.
fn helo_loop(
    socket: &UdpSocket,
    expected: &RwLock<SocketAddr>,
    session_flag: u8,
    interval: Duration,
    counters: &AtomicCounters,
    shutdown: &AtomicBool,
) {
    let helo = HeloRequest::new_v2(session_flag).encode();
    while !shutdown.load(Ordering::Relaxed) {
        let target = read_expected_addr(expected);
        match socket.send_to(&helo, target) {
            Ok(_) => {
                counters.helos_sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                counters.send_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        let deadline = Instant::now() + interval;
        while Instant::now() < deadline {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(HELO_SLEEP_STEP);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket as StdUdpSocket;
    use std::time::Instant;

    use rsh_protocol::StreamFlags;
    use tokio::time::sleep;

    use super::*;

    /// Fake board: one loopback socket that sends stream/control datagrams
    /// and receives HELOs, like the real board's single UDP endpoint.
    ///
    /// The socket is `connect()`ed to the transport and uses `send()`, not
    /// `send_to()`: this sandbox blocks the `sendto`/`sendmsg` syscalls
    /// with `EPERM` while connected `send()` works, so the fake board
    /// must use the connected form. (The transport itself keeps `send_to`,
    /// which is correct on real machines — see the ignored end-to-end
    /// tests.)
    struct FakeBoard {
        /// Board-side socket (bound to loopback, ephemeral port).
        socket: StdUdpSocket,
    }

    impl FakeBoard {
        /// Bind a fake board on `ip` (loopback, ephemeral port).
        fn new(ip: IpAddr) -> Self {
            let socket = StdUdpSocket::bind(SocketAddr::new(ip, 0)).expect("fake board binds");
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("fake board read timeout");
            Self { socket }
        }

        /// The board's address (what the transport must expect).
        fn addr(&self) -> SocketAddr {
            self.socket.local_addr().expect("fake board has an address")
        }

        /// Connect the board socket to the transport (enables `send()`).
        fn connect_to(&self, target: SocketAddr) {
            self.socket.connect(target).expect("fake board connects");
        }

        /// Send raw bytes toward the transport (requires `connect_to`).
        fn send(&self, bytes: &[u8]) {
            self.socket.send(bytes).expect("fake board sends");
        }

        /// Send one stream frame toward the transport.
        fn send_stream(&self, frame: &StreamFrame) {
            self.send(&frame.encode());
        }
    }

    /// Test config: the fake board's single socket is HELO + control target.
    fn test_config(board_ip: IpAddr, board_port: u16) -> UdpConfig {
        UdpConfig {
            board_ip,
            helo_port: board_port,
            control_port: board_port,
            local_port: 0,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            helo_interval: Duration::from_secs(1),
            session_flag: 0,
        }
    }

    /// A stream frame with steering on slot 0 and two encoders.
    fn test_frame(seq: u16) -> StreamFrame {
        StreamFrame::new(
            seq,
            u32::from(seq) * 5_000,
            StreamFlags::default(),
            &[(0, 1650)],
            &[10, -5],
            0b101,
        )
    }

    /// Give the reader thread time to process what was just sent.
    async fn settle() {
        sleep(Duration::from_millis(300)).await;
    }

    #[test]
    fn second_bind_on_same_port_fails_addr_in_use() {
        // F21: no `SO_REUSEADDR` by design — a second bind on a held
        // port must fail loud with typed `AddrInUse`, never silently
        // share the port with the first holder (on Windows reuse
        // would let two RSH processes race for the board's stream).
        let first = StdUdpSocket::bind("127.0.0.1:0").expect("first binds");
        let port = first.local_addr().expect("addr").port();
        let config = UdpConfig {
            local_port: port,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            ..UdpConfig::default()
        };
        let error = UdpTransport::bind_typed(config).expect_err("second bind must fail");
        assert!(
            matches!(error, UdpBindError::AddrInUse),
            "typed AddrInUse, got {error:?}"
        );
        assert_eq!(error.code(), "AddrInUse");
    }

    #[test]
    fn transport_is_send() {
        /// Compile-time assertion: the transport must be spawnable.
        fn assert_send<T: Send>() {}
        assert_send::<UdpTransport>();
    }

    #[test]
    fn reader_and_helo_threads_alive_then_stop_on_shutdown() {
        // M6/L11: handles are stored and pollable; shutdown wakes the
        // reader out of recv promptly (socket shutdown, not just the
        // 100 ms read timeout).
        let config = UdpConfig {
            local_port: 0, // ephemeral: no clash with other tests
            ..UdpConfig::default()
        };
        let transport = UdpTransport::bind(config).expect("bind succeeds");
        assert!(transport.reader_alive(), "reader running after bind");
        assert!(transport.helo_alive(), "helo running after bind");
        assert_eq!(transport.counters().reader_panics, 0);
        assert_eq!(transport.counters().helo_panics, 0);
        let started = Instant::now();
        transport.shutdown();
        let deadline = Instant::now() + Duration::from_secs(2);
        while (transport.reader_alive() || transport.helo_alive()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!transport.reader_alive(), "reader stopped after shutdown");
        assert!(!transport.helo_alive(), "helo stopped after shutdown");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn frames_delivered_counts_only_channel_accepted_frames() {
        // L10: a parsed frame dropped by a full channel is an
        // overflow, not a delivery.
        let (frame_tx, _frame_rx) = mpsc::channel(1);
        let (control_tx, _control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
        let (hello_tx, _hello_rx) = mpsc::channel(HELLO_CHANNEL_CAPACITY);
        let (status_tx, _status_rx) = mpsc::channel(STATUS_CHANNEL_CAPACITY);
        let senders = DemuxSenders {
            frame: frame_tx,
            control: control_tx,
            hello: hello_tx,
            status: status_tx,
        };
        let counters = AtomicCounters::default();
        // Fill the 1-capacity frame channel with a first frame.
        let first = StreamFrame::new(1, 5_000, StreamFlags::default(), &[(0, 1650)], &[], 0);
        handle_datagram(&first.encode(), &senders, &counters);
        assert_eq!(counters.snapshot().frames_delivered, 1);
        // Second frame: parses fine but the channel is full.
        let second = StreamFrame::new(2, 10_000, StreamFlags::default(), &[(0, 1650)], &[], 0);
        handle_datagram(&second.encode(), &senders, &counters);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.frames_delivered, 1, "overflow is not a delivery");
        assert_eq!(snapshot.dropped_overflow, 1);
    }

    #[tokio::test]
    async fn stream_frame_delivered_from_expected_sender() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let frame = test_frame(7);
        board.connect_to(transport.local_addr().expect("local addr"));
        board.send_stream(&frame);
        let got = timeout(Duration::from_secs(2), transport.next_frame())
            .await
            .expect("frame arrives in time")
            .expect("source live");
        assert_eq!(got, frame);
        assert_eq!(transport.counters().frames_delivered, 1);
        assert_eq!(transport.counters().datagrams_dropped, 0);
        transport.shutdown();
    }

    #[tokio::test]
    async fn malformed_datagrams_dropped_and_counted() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let target = transport.local_addr().expect("local addr");
        board.connect_to(target);
        board.send(b"garbage");
        board.send(b"RSH2");
        let mut good = test_frame(3).encode();
        let last = good.len() - 1;
        good[last] ^= 0xFF; // corrupt the CRC trailer
        board.send(&good);
        settle().await;
        let counters = transport.counters();
        assert_eq!(counters.datagrams_received, 3);
        assert_eq!(counters.dropped_bad_crc, 1);
        assert_eq!(counters.dropped_malformed, 2);
        assert_eq!(counters.datagrams_dropped, 3);
        assert_eq!(counters.frames_delivered, 0);
        let none = timeout(Duration::from_millis(200), transport.next_frame()).await;
        assert!(none.is_err(), "no frame should arrive, got {none:?}");
        transport.shutdown();
    }

    #[tokio::test]
    async fn wrong_sender_dropped() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let intruder = FakeBoard::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let target = transport.local_addr().expect("local addr");
        intruder.connect_to(target);
        intruder.send_stream(&test_frame(1));
        settle().await;
        assert_eq!(transport.counters().dropped_wrong_sender, 1);
        assert_eq!(transport.counters().datagrams_dropped, 1);
        let none = timeout(Duration::from_millis(200), transport.next_frame()).await;
        assert!(none.is_err(), "spoofed frame must not arrive, got {none:?}");
        transport.shutdown();
    }

    #[tokio::test]
    async fn expected_remote_refresh_accepts_new_sender() {
        // The C# v2.0.4 regression: the filter was never refreshed after
        // the configured IP changed, so every frame from the new board
        // was dropped. Here the refresh must take effect immediately.
        let board_a = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let board_b = FakeBoard::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board_a.addr().port(),
        ))
        .expect("bind");
        let target = transport.local_addr().expect("local addr");
        board_a.connect_to(target);
        board_b.connect_to(target);
        board_a.send_stream(&test_frame(1));
        let first = timeout(Duration::from_secs(2), transport.next_frame())
            .await
            .expect("first frame arrives")
            .expect("source live");
        assert_eq!(first.seq, 1);
        transport.set_expected_remote(board_b.addr());
        assert_eq!(transport.expected_remote(), board_b.addr());
        board_a.send_stream(&test_frame(2));
        board_b.send_stream(&test_frame(3));
        let second = timeout(Duration::from_secs(2), transport.next_frame())
            .await
            .expect("new sender frame arrives")
            .expect("source live");
        assert_eq!(second.seq, 3, "only the new sender is accepted");
        settle().await;
        assert_eq!(transport.counters().dropped_wrong_sender, 1);
        transport.shutdown();
    }

    /// The HELO loop attempts one send per second. On a healthy machine
    /// every attempt lands (`helos_sent`); where the sandbox blocks the
    /// `sendto` syscall the attempts fail instead (`send_errors`) — either
    /// way the 1 Hz cadence is what this test proves. Byte-level delivery
    /// is covered by `helo_received_by_board_at_one_hertz` (ignored here).
    #[tokio::test]
    async fn helo_attempted_at_one_hertz() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        sleep(Duration::from_millis(2600)).await;
        let counters = transport.counters();
        let attempts = counters.helos_sent + counters.send_errors;
        assert!(
            attempts >= 2,
            "HELO attempts in 2.6 s (immediate + 1 Hz): sent={} errors={}",
            counters.helos_sent,
            counters.send_errors
        );
        transport.shutdown();
    }

    /// End-to-end HELO delivery: the board receives a v2 HELO with
    /// `sessionFlag` 0, immediately and then once per second.
    ///
    /// Ignored in this sandbox: it blocks the `sendto`/`sendmsg` syscalls
    /// with `EPERM`, so the transport's HELOs can never leave. Run on
    /// Windows or CI, where UDP `sendto` works.
    #[tokio::test]
    #[cfg_attr(
        not(windows),
        ignore = "sandbox blocks UDP sendto (EPERM); run where UDP send works"
    )]
    async fn helo_received_by_board_at_one_hertz() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let socket = board.socket.try_clone().expect("fake board socket clones");
        let (count, first) = tokio::task::spawn_blocking(move || {
            let mut count = 0usize;
            let mut first: Option<Vec<u8>> = None;
            let mut buf = [0u8; 64];
            let deadline = Instant::now() + Duration::from_millis(2600);
            while Instant::now() < deadline {
                match socket.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        count += 1;
                        if first.is_none() {
                            first = Some(buf[..n].to_vec());
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("fake board recv failed: {e}"),
                }
            }
            (count, first)
        })
        .await
        .expect("receiver task joins");
        assert!(count >= 2, "HELOs in 2.6 s (immediate + 1 Hz): {count}");
        assert_eq!(
            first.as_deref(),
            Some(HeloRequest::new_v2(0).encode().as_slice()),
            "HELO is v2, sessionFlag 0"
        );
        assert!(
            transport.counters().helos_sent >= 2,
            "helos_sent counter: {}",
            transport.counters().helos_sent
        );
        transport.shutdown();
    }

    /// Control send + reply round trip through the transport.
    ///
    /// Ignored in this sandbox: it blocks the `sendto`/`sendmsg` syscalls
    /// with `EPERM`, so the transport's control send can never leave. Run
    /// on Windows or CI, where UDP `sendto` works.
    #[tokio::test]
    #[cfg_attr(
        not(windows),
        ignore = "sandbox blocks UDP sendto (EPERM); run where UDP send works"
    )]
    async fn control_send_and_reply_round_trip() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let socket = board.socket.try_clone().expect("fake board socket clones");
        // Fake board: ignore HELOs, answer the first RSHC datagram.
        let replier = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; MAX_DATAGRAM_LEN];
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                match socket.recv_from(&mut buf) {
                    Ok((n, src)) if buf[..n].starts_with(b"RSHC") => {
                        socket.connect(src).expect("fake board connects back");
                        socket.send(b"RSHC-REPLY").expect("fake board replies");
                        return n;
                    }
                    Ok(_) => {}
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("fake board recv failed: {e}"),
                }
            }
            panic!("no control request arrived");
        });
        transport
            .send_control(b"RSHC-request")
            .await
            .expect("control send");
        let reply = transport
            .recv_control(Duration::from_secs(2))
            .await
            .expect("control reply arrives");
        assert_eq!(reply, b"RSHC-REPLY");
        assert_eq!(transport.counters().control_received, 1);
        replier.await.expect("replier joins");
        transport.shutdown();
    }

    #[tokio::test]
    async fn control_reply_from_wrong_sender_dropped() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let intruder = FakeBoard::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let target = transport.local_addr().expect("local addr");
        intruder.connect_to(target);
        intruder.send(b"RSHC-spoof");
        let reply = transport.recv_control(Duration::from_millis(300)).await;
        assert!(
            matches!(reply, Err(TransportError::Timeout)),
            "spoofed reply must time out, got {reply:?}"
        );
        assert_eq!(transport.counters().dropped_wrong_sender, 1);
        transport.shutdown();
    }

    #[tokio::test]
    async fn sustains_200_pps_for_two_seconds() {
        let board = FakeBoard::new(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let mut transport = UdpTransport::bind(test_config(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            board.addr().port(),
        ))
        .expect("bind");
        let target = transport.local_addr().expect("local addr");
        board.connect_to(target);
        let socket = board.socket.try_clone().expect("fake board socket clones");
        let sender = thread::spawn(move || {
            for seq in 0..400u16 {
                let frame = test_frame(seq);
                socket.send(&frame.encode()).expect("fake board sends");
                thread::sleep(Duration::from_millis(5));
            }
        });
        let mut received = 0u16;
        let deadline = Instant::now() + Duration::from_secs(5);
        while received < 400 && Instant::now() < deadline {
            match timeout(Duration::from_millis(500), transport.next_frame()).await {
                Ok(Some(frame)) => {
                    assert_eq!(frame.seq, received, "frames in order, none lost");
                    received += 1;
                }
                Ok(None) => panic!("source dropped after {received} frames"),
                Err(e) => panic!("timed out after {received} of 400 frames: {e}"),
            }
        }
        sender.join().expect("sender thread joins");
        assert_eq!(received, 400, "all 200 pps x 2 s frames delivered");
        let counters = transport.counters();
        assert_eq!(counters.frames_delivered, 400);
        assert_eq!(counters.datagrams_dropped, 0);
        transport.shutdown();
    }
}
